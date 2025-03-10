use crate::constants;
use crate::context::{Context, ContractAttributes};
use crate::errors::{AlreadyDefined, SemanticError};
use crate::namespace::events::EventDef;
use crate::namespace::scopes::{ContractScope, ModuleScope, Scope, Shared};
use crate::namespace::types::{Contract, FixedSize, Type};
use crate::traversal::{functions, types};
use fe_common::diagnostics::Label;
use fe_common::utils::humanize::pluralize_conditionally;
use fe_parser::ast as fe;
use fe_parser::node::Node;
use std::rc::Rc;

/// Gather context information for contract definitions and check for type
/// errors.
pub fn contract_def(
    module_scope: Shared<ModuleScope>,
    context: &mut Context,
    stmt: &Node<fe::ModuleStmt>,
) -> Result<Shared<ContractScope>, SemanticError> {
    if let fe::ModuleStmt::ContractDef {
        name,
        fields: _,
        body,
    } = &stmt.kind
    {
        let contract_scope = ContractScope::new(&name.kind, Rc::clone(&module_scope));

        // Contract fields are evaluated in the next pass together with function bodies
        // so that they can use other contract types that may only be defined after the
        // current contract.

        for stmt in body {
            match &stmt.kind {
                fe::ContractStmt::EventDef { .. } => {
                    event_def(Rc::clone(&contract_scope), context, stmt)
                }
                fe::ContractStmt::FuncDef { .. } => {
                    functions::func_def(Rc::clone(&contract_scope), context, stmt)
                }
            }
            .map_err(|error| error.with_context(stmt.span))?;
        }

        let contract_attributes = ContractAttributes::from(Rc::clone(&contract_scope));

        if let Err(AlreadyDefined) = contract_scope
            .borrow()
            .module_scope()
            .borrow_mut()
            .add_type_def(
                &name.kind,
                Type::Contract(Contract {
                    name: name.kind.to_string(),
                    functions: contract_attributes.public_functions,
                }),
            )
        {
            context.fancy_error(
                "a contract with the same name already exists",
                // TODO: figure out how to include the previously defined contract
                vec![Label::primary(
                    stmt.span,
                    format!("Conflicting definition of contract `{}`", name.kind),
                )],
                vec![format!(
                    "Note: Give one of the `{}` contracts a different name",
                    name.kind
                )],
            )
        }

        return Ok(contract_scope);
    }

    unreachable!()
}

/// Gather context information for fields and function bodies of contracts.
/// Gathering this information is deferred to allow contracts to refer to other
/// contract types that are defined after it.
pub fn contract_body(
    contract_scope: Shared<ContractScope>,
    context: &mut Context,
    stmt: &Node<fe::ModuleStmt>,
) -> Result<(), SemanticError> {
    if let fe::ModuleStmt::ContractDef { fields, body, .. } = &stmt.kind {
        for field in fields {
            contract_field(Rc::clone(&contract_scope), context, field)?;
        }

        for stmt in body {
            if let fe::ContractStmt::FuncDef { .. } = &stmt.kind {
                functions::func_body(Rc::clone(&contract_scope), context, stmt)
                    .map_err(|error| error.with_context(stmt.span))?;
            };
        }

        let contract_attributes = ContractAttributes::from(Rc::clone(&contract_scope));

        context.add_contract(stmt, contract_attributes);

        return Ok(());
    }

    unreachable!()
}

fn contract_field(
    scope: Shared<ContractScope>,
    context: &mut Context,
    stmt: &Node<fe::Field>,
) -> Result<(), SemanticError> {
    let fe::Field { name, typ, .. } = &stmt.kind;
    let typ = types::type_desc(&Scope::Contract(Rc::clone(&scope)), context, &typ)?;

    if let Err(AlreadyDefined) = scope.borrow_mut().add_field(&name.kind, typ) {
        context.fancy_error(
            "a contract field with the same name already exists",
            // TODO: figure out how to include the previously defined field
            vec![Label::primary(
                stmt.span,
                format!("Conflicting definition of field `{}`", name.kind),
            )],
            vec![format!(
                "Note: Give one of the `{}` fields a different name",
                name.kind
            )],
        )
    }

    Ok(())
}

fn event_def(
    scope: Shared<ContractScope>,
    context: &mut Context,
    stmt: &Node<fe::ContractStmt>,
) -> Result<(), SemanticError> {
    if let fe::ContractStmt::EventDef { name, fields } = &stmt.kind {
        let name = &name.kind;

        let (is_indexed_bools, all_fields): (Vec<bool>, Vec<(String, FixedSize)>) = fields
            .iter()
            .map(|field| event_field(Rc::clone(&scope), context, field))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .unzip();

        let indexed_fields = is_indexed_bools
            .into_iter()
            .enumerate()
            .filter(|(_, is_indexed)| *is_indexed)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        if indexed_fields.len() > constants::MAX_INDEXED_EVENT_FIELDS {
            let excess_count = indexed_fields.len() - constants::MAX_INDEXED_EVENT_FIELDS;

            let labels = fields
                .iter()
                .enumerate()
                .filter_map(|(idx, field)| {
                    if indexed_fields.contains(&idx) {
                        Some(field)
                    } else {
                        None
                    }
                })
                .map(|field| Label::primary(field.span, "Indexed field"))
                .collect();

            context.fancy_error(
                "More than three indexed fields.",
                labels,
                vec![format!(
                    "Note: Remove the `idx` keyword from at least {} {}.",
                    excess_count,
                    pluralize_conditionally("field", excess_count)
                )],
            );
        }

        // check if they are trying to index an array type
        // todo clean all this up
        for index in indexed_fields.clone() {
            match all_fields[index].1.to_owned() {
                FixedSize::Base(_) => {}
                _ => context.not_yet_implemented("non-base type indexed event fields", stmt.span),
            }
        }

        let event = EventDef::new(name, all_fields, indexed_fields);

        context.add_event(stmt, event.clone());

        if let Err(AlreadyDefined) = scope.borrow_mut().add_event(name, event) {
            context.fancy_error(
                "an event with the same name already exists",
                // TODO: figure out how to include the previously defined event
                vec![Label::primary(
                    stmt.span,
                    format!("Conflicting definition of event `{}`", name),
                )],
                vec![format!(
                    "Note: Give one of the `{}` events a different name",
                    name
                )],
            )
        }

        return Ok(());
    }

    unreachable!()
}

fn event_field(
    scope: Shared<ContractScope>,
    context: &mut Context,
    field: &Node<fe::EventField>,
) -> Result<(bool, (String, FixedSize)), SemanticError> {
    let fe::EventField { is_idx, name, typ } = &field.kind;
    Ok((
        *is_idx,
        (
            name.kind.to_string(),
            types::type_desc_fixed_size(&Scope::Contract(scope), context, &typ)?,
        ),
    ))
}
