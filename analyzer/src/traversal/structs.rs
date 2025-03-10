use fe_common::diagnostics::Label;
use fe_parser::ast as fe;
use fe_parser::{ast::Field, node::Node};

use crate::errors::{AlreadyDefined, SemanticError};
use crate::namespace::scopes::{ModuleScope, Scope, Shared};
use crate::namespace::types::{FixedSize, Struct, Type};
use crate::traversal::types::type_desc;
use crate::Context;
use std::rc::Rc;

pub fn struct_def(
    context: &mut Context,
    module_scope: Shared<ModuleScope>,
    stmt: &Node<fe::ModuleStmt>,
) -> Result<(), SemanticError> {
    if let fe::ModuleStmt::StructDef { name, fields } = &stmt.kind {
        let mut val = Struct::new(&name.kind);
        for field in fields {
            let Field { name, typ, .. } = &field.kind;
            let field_type = type_desc(&Scope::Module(Rc::clone(&module_scope)), context, typ)?;
            if let Type::Base(base_typ) = field_type {
                if let Err(AlreadyDefined) = val.add_field(&name.kind, &FixedSize::Base(base_typ)) {
                    let first_definition = fields
                        .iter()
                        .find(|val| {
                            let Field {
                                name: inner_name, ..
                            } = &val.kind;
                            inner_name.kind == name.kind && val.span != field.span
                        })
                        .expect("Missing field");

                    context.fancy_error(
                        "a struct field with the same name already exists",
                        vec![
                            Label::primary(
                                first_definition.span,
                                format!("First definition of field `{}`", name.kind),
                            ),
                            Label::primary(
                                field.span,
                                format!("Conflicting definition of field `{}`", name.kind),
                            ),
                        ],
                        vec![format!(
                            "Note: Give one of the `{}` fields a different name",
                            name.kind
                        )],
                    )
                }
            } else {
                context.not_yet_implemented("non-base type struct fields", field.span)
            }
        }
        if let Err(AlreadyDefined) = module_scope
            .borrow_mut()
            .add_type_def(&name.kind, Type::Struct(val))
        {
            context.fancy_error(
                "a struct with the same name already exists",
                // TODO: figure out how to include the previously defined struct
                vec![Label::primary(
                    stmt.span,
                    format!("Conflicting definition of struct `{}`", name.kind),
                )],
                vec![format!(
                    "Note: Give one of the `{}` structs a different name",
                    name.kind
                )],
            )
        }

        return Ok(());
    }
    unreachable!()
}
