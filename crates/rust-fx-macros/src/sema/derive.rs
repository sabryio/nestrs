use syn::{Data, DeriveInput, Fields};

use crate::diagnostics::Diagnostics;
use crate::ir::{ComponentMarker, Params, ParamsShape};

pub(crate) fn component(input: &DeriveInput, _diagnostics: &mut Diagnostics) -> ComponentMarker {
    ComponentMarker { input: input.clone() }
}

pub(crate) fn params(input: &DeriveInput, diagnostics: &mut Diagnostics) -> Params {
    if !input.generics.params.is_empty() {
        diagnostics.error(
            &input.generics,
            "#[derive(fx::Params)] does not support generic structs",
        );
    }
    let shape = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => ParamsShape::Named(
                fields
                    .named
                    .iter()
                    .filter_map(|field| Some((field.ident.clone()?, field.ty.clone())))
                    .collect(),
            ),
            Fields::Unnamed(fields) => {
                ParamsShape::Tuple(fields.unnamed.iter().map(|field| field.ty.clone()).collect())
            }
            Fields::Unit => ParamsShape::Unit,
        },
        _ => {
            diagnostics.error(&input.ident, "#[derive(fx::Params)] needs a struct");
            ParamsShape::Unit
        }
    };
    Params {
        name: input.ident.clone(),
        shape,
    }
}
