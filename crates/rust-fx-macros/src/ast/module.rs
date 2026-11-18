use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, ItemStruct, LitStr, Token, Type, bracketed};

/// `Type` or `Type as "name"` in `exports`.
pub(crate) struct ExportEntry {
    pub(crate) ty: Type,
    pub(crate) name: Option<LitStr>,
}

impl Parse for ExportEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse()?;
        let name = if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok(Self { ty, name })
    }
}

/// The value of one `key = value` argument.
pub(crate) enum Value {
    Name(LitStr),
    Imports(Vec<Type>),
    Providers(Vec<Expr>),
    Invokes(Vec<Expr>),
    Decorators(Vec<Expr>),
    Exports(Vec<ExportEntry>),
}

pub(crate) struct Entry {
    pub(crate) key: Ident,
    pub(crate) value: Value,
}

pub(crate) struct ModuleAst {
    pub(crate) item: ItemStruct,
    /// In the order written, so that analysis can point at duplicates.
    pub(crate) entries: Vec<Entry>,
}

fn list<T: Parse>(input: ParseStream) -> syn::Result<Vec<T>> {
    let content;
    bracketed!(content in input);
    Ok(Punctuated::<T, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect())
}

struct Entries(Vec<Entry>);

impl Parse for Entries {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value = match key.to_string().as_str() {
                "name" => Value::Name(input.parse()?),
                "imports" => Value::Imports(list(input)?),
                "providers" => Value::Providers(list(input)?),
                "invokes" => Value::Invokes(list(input)?),
                "decorators" => Value::Decorators(list(input)?),
                "exports" => Value::Exports(list(input)?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown key `{other}`; expected name, imports, providers, invokes, decorators or exports"
                        ),
                    ));
                }
            };
            entries.push(Entry { key, value });
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self(entries))
    }
}

pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<ModuleAst> {
    Ok(ModuleAst {
        item: syn::parse2(item)?,
        entries: syn::parse2::<Entries>(args)?.0,
    })
}
