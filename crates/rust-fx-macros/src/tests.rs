//! Unit tests for the pipeline stages, run on token strings without compiling the output.

use proc_macro2::TokenStream;
use quote::quote;

use crate::macros::{BindMacro, ComponentMacro, MainMacro, ModuleMacro, ParamsMacro};
use crate::pipeline::{Context, Macro, expand};

fn cx() -> Context {
    Context { fx: quote!(::fx) }
}

/// Runs a macro and returns its output as a whitespace-free string.
fn run<M: Macro>(args: TokenStream, item: TokenStream) -> String {
    expand::<M>(args, item, &cx()).to_string().replace(' ', "")
}

fn errors(output: &str) -> usize {
    output.matches("compile_error").count()
}

#[test]
fn component_registers_constructors_hooks_invokes_and_decorators() {
    let out = run::<ComponentMacro>(
        quote!(),
        quote! {
            impl Clock {
                #[fx::constructor(group = "clocks")]
                pub fn new() -> Self { Self }
                #[fx::on_start]
                async fn open(&self) {}
                #[fx::on_stop]
                fn close(&self) {}
                #[fx::invoke]
                fn warm(_c: std::sync::Arc<Clock>) {}
                #[fx::decorate]
                fn wrap(c: std::sync::Arc<Clock>) -> std::sync::Arc<Clock> { c }
            }
        },
    );
    assert_eq!(errors(&out), 0, "{out}");
    assert!(out.contains(".constructor(::fx::constructor(<Clock>::new)"), "{out}");
    assert!(out.contains(".in_group_named(\"clocks\")"), "{out}");
    assert!(out.contains(".on_start(") && out.contains(".on_stop("), "{out}");
    assert!(
        out.contains(".invoke(<Clock>::warm)") && out.contains(".decorate(<Clock>::wrap)"),
        "{out}"
    );
    assert!(!out.contains("fx::constructor]"), "our attributes are removed: {out}");
}

#[test]
fn component_reports_every_violation_at_once_and_keeps_the_item() {
    let out = run::<ComponentMacro>(
        quote!(),
        quote! {
            impl Clock {
                #[fx::constructor]
                pub fn new(self) -> Self { Self }
                #[fx::on_start]
                fn open(self) {}
                #[fx::constructor(name = "a", group)]
                fn other() -> Self { Self }
            }
        },
    );
    assert_eq!(errors(&out), 3, "{out}");
    assert!(
        out.contains("implClock"),
        "the item survives so one mistake does not cascade: {out}"
    );
}

#[test]
fn component_rejects_trait_impls_generics_and_orphan_hooks() {
    let trait_impl = run::<ComponentMacro>(quote!(), quote!(impl Tr for Clock {}));
    assert!(trait_impl.contains("use#[fx::bind]"), "{trait_impl}");

    let generic = run::<ComponentMacro>(
        quote!(),
        quote!(
            impl<T> Clock<T> {}
        ),
    );
    assert!(generic.contains("doesnotsupportgenericimpls"), "{generic}");

    let orphan = run::<ComponentMacro>(quote!(), quote! { impl Clock { #[fx::on_stop] fn close(&self) {} } });
    assert!(orphan.contains("needa`#[fx::constructor]`"), "{orphan}");
}

#[test]
fn hooks_and_invokes_take_no_arguments() {
    let out = run::<ComponentMacro>(quote!(), quote! { impl Clock { #[fx::invoke(x)] fn go() {} } });
    assert!(out.contains("takesnoarguments"), "{out}");
}

#[test]
fn bind_generates_the_bound_impl_and_the_registration() {
    let out = run::<BindMacro>(
        quote!(group = "jobs"),
        quote!(impl Job for Cleanup { fn run(&self) {} }),
    );
    assert_eq!(errors(&out), 0, "{out}");
    assert!(out.contains("impl::fx::Bound<dynJob>forCleanup"), "{out}");
    assert!(
        out.contains("Binding::new::<dynJob,Cleanup>(::fx::__private::Placement::GroupNamed(\"jobs\"))"),
        "{out}"
    );
}

#[test]
fn bind_needs_a_trait_impl_and_no_generics() {
    let inherent = run::<BindMacro>(quote!(), quote!(impl Cleanup {}));
    assert!(inherent.contains("goesonatraitimpl"), "{inherent}");

    let generic = run::<BindMacro>(
        quote!(),
        quote!(
            impl<T> Job for Cleanup<T> {}
        ),
    );
    assert!(generic.contains("doesnotsupportgenericimpls"), "{generic}");

    let both = run::<BindMacro>(quote!(name = "a", group), quote!(impl Job for Cleanup {}));
    assert!(both.contains("eithernamedorinagroup"), "{both}");

    let unknown = run::<BindMacro>(quote!(as_dyn), quote!(impl Job for Cleanup {}));
    assert!(unknown.contains("accepts"), "{unknown}");
}

#[test]
fn module_registers_components_and_single_constructors_differently() {
    let out = run::<ModuleMacro>(
        quote!(imports = [Config], providers = [Clock, make_pool], invokes = [warm], exports = [Clock, Pool as "main"]),
        quote!(
            pub struct M;
        ),
    );
    assert_eq!(errors(&out), 0, "{out}");
    assert!(out.contains(".component::<Clock>()"), "{out}");
    assert!(out.contains(".constructor(make_pool)"), "{out}");
    assert!(out.contains("Export::named::<Pool>(\"main\")"), "{out}");
}

#[test]
fn module_rules_are_all_reported() {
    let out = run::<ModuleMacro>(
        quote!(
            providers = [Clock, Clock],
            providers = [Other],
            invokes = [Clock],
            exports = [A, A]
        ),
        quote!(
            pub struct M;
        ),
    );
    // duplicate provider, duplicate key, component under invokes, duplicate export
    assert_eq!(errors(&out), 4, "{out}");
}

#[test]
fn module_unknown_key_is_a_syntax_error_with_the_valid_keys() {
    let out = run::<ModuleMacro>(
        quote!(provides = [A]),
        quote!(
            pub struct M;
        ),
    );
    assert!(out.contains("unknownkey`provides`"), "{out}");
}

#[test]
fn params_handles_every_struct_shape_and_rejects_the_rest() {
    let named = run::<ParamsMacro>(
        quote!(),
        quote!(
            struct D {
                a: Arc<A>,
                b: Option<Arc<B>>,
            }
        ),
    );
    assert!(named.contains("a:<Arc<A>as::fx::Extract>::extract"), "{named}");

    let tuple = run::<ParamsMacro>(
        quote!(),
        quote!(
            struct D(Arc<A>);
        ),
    );
    assert!(tuple.contains("Self(<Arc<A>"), "{tuple}");

    let en = run::<ParamsMacro>(
        quote!(),
        quote!(
            enum D {
                A,
            }
        ),
    );
    assert!(en.contains("needsastruct"), "{en}");

    let generic = run::<ParamsMacro>(
        quote!(),
        quote!(
            struct D<T>(T);
        ),
    );
    assert!(generic.contains("doesnotsupportgenericstructs"), "{generic}");
}

#[test]
fn port_adds_supertraits_and_generates_the_alias() {
    use crate::macros::PortMacro;
    let out = run::<PortMacro>(
        quote!(),
        quote!(
            pub trait Clock {
                fn tick(&self) -> u32;
            }
        ),
    );
    assert_eq!(errors(&out), 0, "{out}");
    assert!(
        out.contains("traitClock:::core::marker::Send+::core::marker::Sync"),
        "{out}"
    );
    assert!(out.contains("pubtypeClockPort=dynClock;"), "{out}");

    let named = run::<PortMacro>(
        quote!(Clocks),
        quote!(
            trait Clock: Send + Sync {}
        ),
    );
    assert!(named.contains("typeClocks=dynClock;"), "{named}");
    assert!(
        !named.contains("::core::marker"),
        "existing bounds are kept as written: {named}"
    );
}

#[test]
fn port_rejects_generic_traits_and_alias_clashes() {
    use crate::macros::PortMacro;
    let generic = run::<PortMacro>(
        quote!(),
        quote!(
            trait Repo<T> {}
        ),
    );
    assert!(generic.contains("doesnotsupportgenerictraits"), "{generic}");
    let clash = run::<PortMacro>(
        quote!(Clock),
        quote!(
            trait Clock {}
        ),
    );
    assert!(clash.contains("mustdifferfromthetrait"), "{clash}");
}

#[test]
fn main_delegates_to_the_runtime_fx_re_exports() {
    let out = run::<MainMacro>(
        quote!(flavor = "current_thread"),
        quote! { async fn main() -> fx::Result<()> { Ok(()) } },
    );
    assert_eq!(errors(&out), 0, "{out}");
    assert!(out.contains("::fx::__private::tokio::main"), "{out}");
    assert!(out.contains("crate=\"::fx::__private::tokio\""), "{out}");
    assert!(out.contains("flavor=\"current_thread\""), "{out}");
}

#[test]
fn main_reports_every_problem_at_once() {
    let out = run::<MainMacro>(quote!(crate = "other"), quote! { fn main(x: u8) {} });
    assert_eq!(errors(&out), 3, "{out}");
    assert!(out.contains("asyncfn"), "{out}");
    assert!(out.contains("takesnoarguments"), "{out}");
    assert!(out.contains("reserved") || out.contains("issetby"), "{out}");
    // An empty stand-in keeps the name defined, so one mistake does not cause a cascade.
    assert!(out.contains("fnmain(){}"), "{out}");
}
