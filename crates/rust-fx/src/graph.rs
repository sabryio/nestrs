//! The planner: flattens options into scopes and nodes, resolves every dependency,
//! and rejects a broken graph before anything runs.
//!
//! Terminology:
//!
//! - A **scope** is the app root or one module.
//! - A **node** is one value to build (a provider, or a decorator applied on top of
//!   another node) or one invoke.
//! - Looking up a key from a scope finds the provider the scope can see (its own, an
//!   imported module's export, or the root's), then threads it through the decorators
//!   that apply, producing a chain of nodes.

use std::any::TypeId;
use std::collections::HashMap;

use crate::descriptor::{Decorator, Invoke, Provider};
use crate::error::{Error, Result};
use crate::extract::{DepKind, DepSpec};
use crate::key::{Key, Origin, shorten};
use crate::module::ModuleRef;
use crate::options::{Item, Options, Replacement};
use crate::value::Stored;

pub(crate) type NodeId = usize;
pub(crate) type ScopeId = usize;
const ROOT: ScopeId = 0;

struct Scope {
    name: String,
    imports: Vec<ScopeId>,
    exports: Vec<Key>,
}

/// How a dependency of a node was resolved.
#[derive(Clone)]
pub(crate) enum Binding {
    One(NodeId),
    Maybe(Option<NodeId>),
    Many(Vec<NodeId>),
}

pub(crate) enum NodeKind {
    Provider(usize),
    Decorator { entry: usize, prev: NodeId },
    Invoke(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    New,
    Visiting,
    Done,
}

pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    scope: ScopeId,
    pub(crate) key: Option<Key>,
    pub(crate) args: Vec<Binding>,
    pub(crate) deps: Vec<NodeId>,
    mark: Mark,
}

pub(crate) struct ProviderEntry {
    scope: ScopeId,
    pub(crate) provider: Provider,
}

pub(crate) struct DecoratorEntry {
    scope: ScopeId,
    pub(crate) decorator: Decorator,
}

pub(crate) struct InvokeEntry {
    scope: ScopeId,
    pub(crate) invoke: Invoke,
}

/// The validated graph, ready to run.
pub(crate) struct Plan {
    pub(crate) nodes: Vec<Node>,
    pub(crate) providers: Vec<ProviderEntry>,
    pub(crate) decorators: Vec<DecoratorEntry>,
    pub(crate) invokes: Vec<InvokeEntry>,
    /// The invoke nodes, in registration order.
    pub(crate) invoke_nodes: Vec<NodeId>,
    /// Every key visible from the app root, with the node that serves it.
    pub(crate) root_view: HashMap<Key, NodeId>,
}

impl Plan {
    pub(crate) fn build(options: Options) -> Result<Plan> {
        Planner::default().plan(options)
    }

    /// The nodes that must be built to build `root`, dependencies first, skipping any
    /// node for which `done` is true.
    pub(crate) fn build_order(&self, root: NodeId, done: &[bool]) -> Vec<NodeId> {
        let mut order = Vec::new();
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![(root, false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                order.push(node);
                continue;
            }
            if done[node] || seen[node] {
                continue;
            }
            seen[node] = true;
            stack.push((node, true));
            for &dep in self.nodes[node].deps.iter().rev() {
                stack.push((dep, false));
            }
        }
        order
    }

    pub(crate) fn origin(&self, node: NodeId) -> Origin {
        match self.nodes[node].kind {
            NodeKind::Provider(i) => self.providers[i].provider.origin,
            NodeKind::Decorator { entry, .. } => self.decorators[entry].decorator.origin,
            NodeKind::Invoke(i) => self.invokes[i].invoke.origin,
        }
    }
}

/// Why a lookup failed.
enum Miss {
    Absent,
    Hidden(ScopeId),
    Ambiguous(Vec<ScopeId>),
}

#[derive(Default)]
struct Planner {
    scopes: Vec<Scope>,
    providers: Vec<ProviderEntry>,
    decorators: Vec<DecoratorEntry>,
    invokes: Vec<InvokeEntry>,
    replacements: Vec<Replacement>,
    provided: HashMap<(ScopeId, Key), usize>,
    members: HashMap<Key, Vec<usize>>,
    decorators_by: HashMap<(ScopeId, Key), Vec<usize>>,
    module_scopes: HashMap<TypeId, ScopeId>,
    nodes: Vec<Node>,
    decorator_nodes: HashMap<(usize, NodeId), NodeId>,
}

impl Planner {
    fn plan(mut self, options: Options) -> Result<Plan> {
        self.scopes.push(Scope {
            name: "root".into(),
            imports: Vec::new(),
            exports: Vec::new(),
        });
        self.register(ROOT, options, &mut Vec::new())?;
        self.apply_replacements()?;
        self.validate_exports_and_decorators()?;

        for index in 0..self.providers.len() {
            self.nodes.push(Node {
                kind: NodeKind::Provider(index),
                scope: self.providers[index].scope,
                key: Some(self.providers[index].provider.key),
                args: Vec::new(),
                deps: Vec::new(),
                mark: Mark::New,
            });
        }
        let mut invoke_nodes = Vec::new();
        for index in 0..self.invokes.len() {
            invoke_nodes.push(self.nodes.len());
            self.nodes.push(Node {
                kind: NodeKind::Invoke(index),
                scope: self.invokes[index].scope,
                key: None,
                args: Vec::new(),
                deps: Vec::new(),
                mark: Mark::New,
            });
        }

        // Validate everything, not just what the invokes reach: a provider with an
        // unmet dependency is a bug even if nobody uses it yet.
        let total = self.nodes.len();
        for node in 0..total {
            self.expand(node, &mut Vec::new())?;
        }

        let root_view = self.root_view()?;
        Ok(Plan {
            nodes: self.nodes,
            providers: self.providers,
            decorators: self.decorators,
            invokes: self.invokes,
            invoke_nodes,
            root_view,
        })
    }

    // ---- registration -------------------------------------------------------------

    fn register(&mut self, scope: ScopeId, options: Options, stack: &mut Vec<ModuleRef>) -> Result<()> {
        for item in options.items {
            match item {
                Item::Provide(provider) => self.add_provider(scope, provider)?,
                Item::Invoke(invoke) => self.invokes.push(InvokeEntry { scope, invoke }),
                Item::Decorate(decorator) => {
                    let index = self.decorators.len();
                    self.decorators_by
                        .entry((scope, decorator.key))
                        .or_default()
                        .push(index);
                    self.decorators.push(DecoratorEntry { scope, decorator });
                }
                Item::Module(module) => {
                    let imported = self.add_module(module, stack)?;
                    if !self.scopes[scope].imports.contains(&imported) {
                        self.scopes[scope].imports.push(imported);
                    }
                }
                Item::Replace(replacement) => self.replacements.push(replacement),
            }
        }
        Ok(())
    }

    fn add_module(&mut self, module: ModuleRef, stack: &mut Vec<ModuleRef>) -> Result<ScopeId> {
        if let Some(position) = stack.iter().position(|m| m.id == module.id) {
            let mut path: Vec<String> = stack[position..].iter().map(|m| shorten(m.name)).collect();
            path.push(shorten(module.name));
            return Err(Error::ModuleCycle { path });
        }
        if let Some(&scope) = self.module_scopes.get(&module.id) {
            return Ok(scope);
        }

        stack.push(module);
        let definition = (module.definition)();
        let scope = self.scopes.len();
        self.scopes.push(Scope {
            name: definition.name.to_string(),
            imports: Vec::new(),
            exports: definition.exports,
        });
        for import in definition.imports {
            let imported = self.add_module(import, stack)?;
            if !self.scopes[scope].imports.contains(&imported) {
                self.scopes[scope].imports.push(imported);
            }
        }
        self.register(scope, definition.options, stack)?;
        stack.pop();

        self.module_scopes.insert(module.id, scope);
        Ok(scope)
    }

    fn add_provider(&mut self, scope: ScopeId, provider: Provider) -> Result<()> {
        let index = self.providers.len();
        let key = provider.key;
        if key.is_group() {
            self.members.entry(key).or_default().push(index);
        } else if let Some(&first) = self.provided.get(&(scope, key)) {
            return Err(Error::Duplicate {
                key,
                scope: self.scopes[scope].name.as_str().into(),
                first: self.providers[first].provider.origin.to_string().into(),
                second: provider.origin.to_string().into(),
            });
        } else {
            self.provided.insert((scope, key), index);
        }
        self.providers.push(ProviderEntry { scope, provider });
        Ok(())
    }

    fn apply_replacements(&mut self) -> Result<()> {
        for replacement in std::mem::take(&mut self.replacements) {
            let mut hit = false;
            for entry in &mut self.providers {
                if entry.provider.key == replacement.key {
                    hit = true;
                    let stored = replacement.value.clone();
                    entry.provider.specs.clear();
                    entry.provider.build = std::sync::Arc::new(move |_, _| {
                        Ok(Box::pin(std::future::ready(Ok::<Stored, _>(stored.clone()))))
                    });
                }
            }
            if !hit {
                return Err(Error::UnknownReplace { key: replacement.key });
            }
            let _ = replacement.location;
        }
        Ok(())
    }

    fn validate_exports_and_decorators(&mut self) -> Result<()> {
        for scope in 0..self.scopes.len() {
            for key in self.scopes[scope].exports.clone() {
                if self.base(scope, key).is_err() {
                    return Err(Error::InvalidExport {
                        module: self.scopes[scope].name.as_str().into(),
                        key,
                    });
                }
            }
        }
        for index in 0..self.decorators.len() {
            let (scope, key) = (self.decorators[index].scope, self.decorators[index].decorator.key);
            let origin = self.decorators[index].decorator.origin;
            if !self.decorators[index].decorator.takes_target() {
                return Err(Error::DecoratorInput {
                    decorator: origin.to_string().into(),
                    key,
                });
            }
            if let Err(miss) = self.base(scope, key) {
                return Err(self.miss_error(miss, key, format!("decorator {origin}"), scope, Vec::new()));
            }
        }
        Ok(())
    }

    // ---- lookup -------------------------------------------------------------------

    /// The value of `key` as a consumer in `scope` sees it, decorators included.
    fn resolve(&mut self, scope: ScopeId, key: Key) -> std::result::Result<NodeId, Miss> {
        let node = self.inner(scope, key)?;
        Ok(if scope == ROOT {
            node
        } else {
            self.decorate(ROOT, key, node)
        })
    }

    /// The value as seen *inside* `scope`: its own decorators, but not the root's.
    fn inner(&mut self, scope: ScopeId, key: Key) -> std::result::Result<NodeId, Miss> {
        let base = self.base(scope, key)?;
        Ok(self.decorate(scope, key, base))
    }

    /// The undecorated provider `scope` can see for `key`.
    fn base(&mut self, scope: ScopeId, key: Key) -> std::result::Result<NodeId, Miss> {
        if let Some(&provider) = self.provided.get(&(scope, key)) {
            return Ok(provider);
        }

        let mut found: Vec<(ScopeId, NodeId)> = Vec::new();
        for import in self.scopes[scope].imports.clone() {
            if !self.scopes[import].exports.contains(&key) {
                continue;
            }
            match self.inner(import, key) {
                Ok(node) if !found.iter().any(|&(_, n)| n == node) => found.push((import, node)),
                Ok(_) | Err(Miss::Absent) => {}
                Err(other) => return Err(other),
            }
        }
        if found.len() > 1 {
            return Err(Miss::Ambiguous(found.into_iter().map(|(s, _)| s).collect()));
        }
        if let Some((_, node)) = found.pop() {
            return Ok(node);
        }

        if scope != ROOT
            && let Some(&provider) = self.provided.get(&(ROOT, key))
        {
            return Ok(provider);
        }

        match self.provided.keys().filter(|(_, k)| *k == key).map(|(s, _)| *s).min() {
            Some(owner) => Err(Miss::Hidden(owner)),
            None => Err(Miss::Absent),
        }
    }

    fn decorate(&mut self, scope: ScopeId, key: Key, base: NodeId) -> NodeId {
        let applicable = self.decorators_by.get(&(scope, key)).cloned().unwrap_or_default();
        let mut current = base;
        for entry in applicable {
            current = *self.decorator_nodes.entry((entry, current)).or_insert_with(|| {
                self.nodes.push(Node {
                    kind: NodeKind::Decorator { entry, prev: current },
                    scope: self.decorators[entry].scope,
                    key: Some(key),
                    args: Vec::new(),
                    deps: Vec::new(),
                    mark: Mark::New,
                });
                self.nodes.len() - 1
            });
        }
        current
    }

    // ---- expansion ----------------------------------------------------------------

    fn specs(&self, node: NodeId) -> Vec<DepSpec> {
        match self.nodes[node].kind {
            NodeKind::Provider(i) => self.providers[i].provider.specs.clone(),
            NodeKind::Decorator { entry, .. } => self.decorators[entry].decorator.specs.clone(),
            NodeKind::Invoke(i) => self.invokes[i].invoke.specs.clone(),
        }
    }

    fn origin(&self, node: NodeId) -> Origin {
        match self.nodes[node].kind {
            NodeKind::Provider(i) => self.providers[i].provider.origin,
            NodeKind::Decorator { entry, .. } => self.decorators[entry].decorator.origin,
            NodeKind::Invoke(i) => self.invokes[i].invoke.origin,
        }
    }

    /// Short label for dependency paths.
    fn path_label(&self, node: NodeId) -> String {
        match (&self.nodes[node].kind, self.nodes[node].key) {
            (NodeKind::Invoke(_), _) | (_, None) => format!("invoke {}", shorten(self.origin(node).name())),
            (NodeKind::Decorator { .. }, Some(key)) => format!("{key} (decorated)"),
            (NodeKind::Provider(_), Some(key)) => key.to_string(),
        }
    }

    fn describe(&self, node: NodeId) -> String {
        let scope = &self.scopes[self.nodes[node].scope].name;
        let origin = self.origin(node);
        match self.nodes[node].kind {
            NodeKind::Invoke(_) => format!("invoke {origin}"),
            _ if self.nodes[node].scope == ROOT => origin.to_string(),
            _ => format!("{origin} in module `{scope}`"),
        }
    }

    fn miss_error(&self, miss: Miss, key: Key, requester: String, scope: ScopeId, path: Vec<String>) -> Error {
        match miss {
            Miss::Absent => Error::Missing {
                key,
                requester: requester.into(),
                path: path.into(),
            },
            Miss::Hidden(owner) => Error::NotVisible {
                key,
                requester: requester.into(),
                provided_in: self.scopes[owner].name.as_str().into(),
                scope: self.scopes[scope].name.as_str().into(),
                path: path.into(),
            },
            Miss::Ambiguous(modules) => Error::Ambiguous {
                key,
                modules: modules.into_iter().map(|s| self.scopes[s].name.clone()).collect(),
            },
        }
    }

    fn expand(&mut self, node: NodeId, stack: &mut Vec<NodeId>) -> Result<()> {
        match self.nodes[node].mark {
            Mark::Done => return Ok(()),
            Mark::Visiting => {
                let start = stack.iter().position(|&n| n == node).unwrap_or(0);
                let mut path: Vec<String> = stack[start..].iter().map(|&n| self.path_label(n)).collect();
                path.push(self.path_label(node));
                return Err(Error::Cycle { path });
            }
            Mark::New => {}
        }
        self.nodes[node].mark = Mark::Visiting;
        stack.push(node);

        let scope = self.nodes[node].scope;
        let target = match self.nodes[node].kind {
            NodeKind::Decorator { prev, .. } => self.nodes[node].key.map(|key| (key, prev)),
            _ => None,
        };

        let mut args = Vec::new();
        let mut deps = Vec::new();
        for spec in self.specs(node) {
            let binding = match spec.kind {
                DepKind::Group => {
                    let members: Vec<NodeId> = self.members.get(&spec.key).cloned().unwrap_or_default();
                    deps.extend(&members);
                    Binding::Many(members)
                }
                DepKind::Required => {
                    let found = match target {
                        Some((key, prev)) if key == spec.key => Ok(prev),
                        _ => self.resolve(scope, spec.key),
                    };
                    match found {
                        Ok(dep) => {
                            deps.push(dep);
                            Binding::One(dep)
                        }
                        Err(miss) => return Err(self.lookup_error(miss, spec.key, node, stack)),
                    }
                }
                DepKind::Optional => match self.resolve(scope, spec.key) {
                    Ok(dep) => {
                        deps.push(dep);
                        Binding::Maybe(Some(dep))
                    }
                    Err(Miss::Absent) => Binding::Maybe(None),
                    Err(miss) => return Err(self.lookup_error(miss, spec.key, node, stack)),
                },
            };
            args.push(binding);
        }

        for &dep in &deps {
            self.expand(dep, stack)?;
        }

        stack.pop();
        let slot = &mut self.nodes[node];
        slot.args = args;
        slot.deps = deps;
        slot.mark = Mark::Done;
        Ok(())
    }

    fn lookup_error(&self, miss: Miss, key: Key, node: NodeId, stack: &[NodeId]) -> Error {
        let path = stack.iter().map(|&n| self.path_label(n)).collect();
        self.miss_error(miss, key, self.describe(node), self.nodes[node].scope, path)
    }

    fn root_view(&mut self) -> Result<HashMap<Key, NodeId>> {
        let mut keys: Vec<Key> = self
            .provided
            .keys()
            .filter(|(scope, _)| *scope == ROOT)
            .map(|(_, key)| *key)
            .collect();
        for import in self.scopes[ROOT].imports.clone() {
            keys.extend(self.scopes[import].exports.iter().copied());
        }

        let mut view = HashMap::new();
        for key in keys {
            if view.contains_key(&key) {
                continue;
            }
            if let Ok(node) = self.resolve(ROOT, key) {
                self.expand(node, &mut Vec::new())?;
                view.insert(key, node);
            }
        }
        Ok(view)
    }
}
