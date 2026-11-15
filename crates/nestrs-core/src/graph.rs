//! Turns a root module into a validated, ordered build plan.
//!
//! Four passes, each failing fast with a precise [`DiError`]:
//!
//! 1. **collect** — walk imports depth-first, reject import cycles and duplicate
//!    providers. Modules land in post-order, so every module has a higher index
//!    than everything it imports.
//! 2. **override** — swap in test replacements, rejecting ones that match nothing.
//! 3. **validate** — compute each module's exports and what it can see (its own
//!    providers, its imports' exports, and every global module's exports), then
//!    check every dependency of every provider against that.
//! 4. **order** — topologically sort providers, rejecting dependency cycles.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use crate::module::ModuleId;
use crate::transport::MountFn;
use crate::{ControllerEntry, DiError, Lifetime, ModuleRef, Overrides, Provider, Token};

pub(crate) struct ControllerMount {
    pub(crate) transport: TypeId,
    pub(crate) mount: MountFn,
}

pub(crate) struct ProviderNode {
    pub(crate) provider: Provider,
    pub(crate) module: usize,
    pub(crate) module_name: &'static str,
    pub(crate) controller: Option<ControllerMount>,
    /// Replaced by a test override: its dependencies must exist, but are exempt
    /// from module visibility.
    overridden: bool,
    /// Assembled by the graph itself (a multi-binding set): reads its members
    /// wherever they live, and is visible to every module.
    synthetic: bool,
}

pub(crate) struct ModuleNode {
    pub(crate) name: &'static str,
    global: bool,
    imports: Vec<usize>,
    exports: Vec<Token>,
}

pub(crate) struct Plan {
    pub(crate) modules: Vec<ModuleNode>,
    pub(crate) providers: Vec<ProviderNode>,
    /// Provider indices, dependencies first.
    pub(crate) order: Vec<usize>,
}

impl Plan {
    pub(crate) fn resolve(root: ModuleRef, overrides: Overrides) -> Result<Self, DiError> {
        let mut collector = Collector::default();
        collector.visit(root)?;
        let Collector {
            modules,
            mut providers,
            owners,
            ..
        } = collector;
        let mut owners = owners;
        synthesize_collections(&mut providers, &mut owners)?;

        for replacement in overrides.into_providers() {
            let token = replacement.token();
            let Some(&index) = owners.get(&token) else {
                return Err(DiError::UnknownOverride { token });
            };
            providers[index].provider = replacement;
            providers[index].overridden = true;
        }

        validate(&modules, &providers, &owners)?;
        let order = topological_order(&providers, &owners)?;
        Ok(Self {
            modules,
            providers,
            order,
        })
    }
}

#[derive(Default)]
struct Collector {
    done: HashMap<ModuleId, usize>,
    stack: Vec<(ModuleId, &'static str)>,
    modules: Vec<ModuleNode>,
    providers: Vec<ProviderNode>,
    /// token -> index into `providers`
    owners: HashMap<Token, usize>,
}

impl Collector {
    fn visit(&mut self, module: ModuleRef) -> Result<usize, DiError> {
        if let Some(&index) = self.done.get(&module.id()) {
            return Ok(index);
        }

        let def = module.define();

        if let Some(start) = self.stack.iter().position(|(id, _)| *id == module.id()) {
            let mut path: Vec<String> = self.stack[start..].iter().map(|(_, name)| (*name).to_owned()).collect();
            path.push(def.name.to_owned());
            return Err(DiError::ModuleCycle { path });
        }

        self.stack.push((module.id(), def.name));
        let mut imports = Vec::with_capacity(def.imports.len());
        for import in &def.imports {
            imports.push(self.visit(import.clone())?);
        }
        self.stack.pop();

        let index = self.modules.len();
        for provider in def.providers {
            self.add(provider, None, index, def.name)?;
        }
        for ControllerEntry {
            provider,
            transport,
            mount,
        } in def.controllers
        {
            self.add(provider, Some(ControllerMount { transport, mount }), index, def.name)?;
        }

        self.modules.push(ModuleNode {
            name: def.name,
            global: def.global,
            imports,
            exports: def.exports,
        });
        self.done.insert(module.id(), index);
        Ok(index)
    }

    fn add(
        &mut self,
        provider: Provider,
        controller: Option<ControllerMount>,
        module: usize,
        module_name: &'static str,
    ) -> Result<(), DiError> {
        let token = provider.token();
        if let Some(&previous) = self.owners.get(&token) {
            // Declaring the same multi-binding twice is harmless and idempotent.
            let declaration = |p: &Provider| p.multi_role().is_some_and(|role| !role.member);
            if declaration(&provider) && declaration(&self.providers[previous].provider) {
                return Ok(());
            }
            return Err(DiError::DuplicateProvider {
                token,
                first: self.providers[previous].module_name,
                second: module_name,
            });
        }
        self.owners.insert(token, self.providers.len());
        self.providers.push(ProviderNode {
            provider,
            module,
            module_name,
            controller,
            overridden: false,
            synthetic: false,
        });
        Ok(())
    }
}

/// Adds one provider per multi-binding, listing every member as a dependency so the
/// set is built after all of them.
fn synthesize_collections(
    providers: &mut Vec<ProviderNode>,
    owners: &mut HashMap<Token, usize>,
) -> Result<(), DiError> {
    let mut groups: Vec<(Token, crate::provider::MultiRole, Vec<Token>)> = Vec::new();
    for node in providers.iter() {
        let Some(role) = node.provider.multi_role() else {
            continue;
        };
        let position = match groups.iter().position(|(target, ..)| *target == role.target) {
            Some(position) => position,
            None => {
                groups.push((role.target, role.clone(), Vec::new()));
                groups.len() - 1
            }
        };
        if role.member {
            groups[position].2.push(node.provider.token());
        }
    }

    for (target, role, members) in groups {
        if let Some(&existing) = owners.get(&target) {
            return Err(DiError::DuplicateProvider {
                token: target,
                first: providers[existing].module_name,
                second: "(multi-binding)",
            });
        }
        owners.insert(target, providers.len());
        providers.push(ProviderNode {
            provider: Provider::collection(target, role.assemble, members),
            module: 0,
            module_name: "(multi-binding)",
            controller: None,
            overridden: false,
            synthetic: true,
        });
    }
    Ok(())
}

fn validate(modules: &[ModuleNode], providers: &[ProviderNode], owners: &HashMap<Token, usize>) -> Result<(), DiError> {
    // What each module owns, what it receives from its imports, and what it exports.
    let mut own: Vec<HashSet<Token>> = Vec::with_capacity(modules.len());
    let mut imported: Vec<HashSet<Token>> = Vec::with_capacity(modules.len());
    let mut exported: Vec<HashSet<Token>> = Vec::with_capacity(modules.len());

    for (index, module) in modules.iter().enumerate() {
        let owned: HashSet<Token> = providers
            .iter()
            .filter(|node| node.module == index)
            .map(|node| node.provider.token())
            .collect();

        let mut from_imports = HashSet::new();
        for &import in &module.imports {
            from_imports.extend(exported[import].iter().copied());
        }

        let mut exports = HashSet::new();
        for token in &module.exports {
            if owned.contains(token) || from_imports.contains(token) {
                exports.insert(*token);
            } else {
                return Err(DiError::InvalidExport {
                    module: module.name,
                    token: *token,
                });
            }
        }

        own.push(owned);
        imported.push(from_imports);
        exported.push(exports);
    }

    // Global modules' exports are reachable from everywhere.
    let globals: HashSet<Token> = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.global)
        .flat_map(|(index, _)| exported[index].iter().copied())
        .collect();

    let collections: HashSet<Token> = providers
        .iter()
        .filter(|node| node.synthetic)
        .map(|node| node.provider.token())
        .collect();

    for node in providers {
        let module = node.module;
        for dependency in node.provider.dependencies() {
            let reachable = if node.overridden || node.synthetic {
                owners.contains_key(dependency)
            } else if collections.contains(dependency) {
                true
            } else {
                own[module].contains(dependency)
                    || imported[module].contains(dependency)
                    || globals.contains(dependency)
            };
            if reachable {
                continue;
            }
            return Err(match owners.get(dependency) {
                Some(&owner) => DiError::NotVisible {
                    requester: node.provider.token(),
                    module: node.module_name,
                    dependency: *dependency,
                    provided_in: providers[owner].module_name,
                },
                None => DiError::MissingProvider {
                    requester: node.provider.token(),
                    module: node.module_name,
                    dependency: *dependency,
                },
            });
        }
    }

    // A singleton must not capture something that is supposed to change per request.
    for node in providers {
        if node.provider.lifetime() != Lifetime::Singleton {
            continue;
        }
        for dependency in node.provider.dependencies() {
            let target = &providers[owners[dependency]].provider;
            if target.lifetime() != Lifetime::Singleton && !node.provider.is_per_request(dependency) {
                return Err(DiError::ScopeMismatch {
                    provider: node.provider.token(),
                    provider_lifetime: node.provider.lifetime(),
                    dependency: *dependency,
                    dependency_lifetime: target.lifetime(),
                });
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    New,
    Active,
    Done,
}

struct Walk<'a> {
    providers: &'a [ProviderNode],
    owners: &'a HashMap<Token, usize>,
    marks: Vec<Mark>,
    stack: Vec<usize>,
    order: Vec<usize>,
}

impl Walk<'_> {
    fn visit(&mut self, index: usize) -> Result<(), DiError> {
        match self.marks[index] {
            Mark::Done => return Ok(()),
            Mark::Active => {
                let start = self.stack.iter().position(|&seen| seen == index).unwrap_or(0);
                let name = |i: usize| self.providers[i].provider.token().to_string();
                let mut path: Vec<String> = self.stack[start..].iter().map(|&i| name(i)).collect();
                path.push(name(index));
                return Err(DiError::CircularDependency { path });
            }
            Mark::New => {}
        }

        self.marks[index] = Mark::Active;
        self.stack.push(index);
        let providers = self.providers;
        for dependency in providers[index].provider.dependencies() {
            // Existence was established by `validate`.
            let next = self.owners[dependency];
            self.visit(next)?;
        }
        self.stack.pop();
        self.marks[index] = Mark::Done;
        self.order.push(index);
        Ok(())
    }
}

fn topological_order(providers: &[ProviderNode], owners: &HashMap<Token, usize>) -> Result<Vec<usize>, DiError> {
    let mut walk = Walk {
        providers,
        owners,
        marks: vec![Mark::New; providers.len()],
        stack: Vec::new(),
        order: Vec::with_capacity(providers.len()),
    };
    for index in 0..providers.len() {
        walk.visit(index)?;
    }
    Ok(walk.order)
}
