//! Build-time policy for trusted peer schemas, never a parser for peer input.
//!
//! Semantic value checks belong in checked domain constructors. This guard keeps
//! schema edits from silently widening the set of representations they accept.

use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorSet};

const PACKAGE: &str = "nockchain.peer.v3";
const TYPE_PREFIX: &str = ".nockchain.peer.v3.";
const FILE_PREFIX: &str = "nockchain/peer/v3/";

pub fn validate_peer_schema(descriptors: &FileDescriptorSet) -> Result<(), String> {
    let mut violations = Vec::new();
    if descriptors.file.is_empty() {
        violations.push("at least one peer schema is required".to_owned());
    }
    for file in &descriptors.file {
        let name = file.name();
        if file.syntax() != "proto3" {
            violations.push(format!("{name}: use proto3 syntax"));
        }
        if file.package() != PACKAGE || !name.starts_with(FILE_PREFIX) {
            violations.push(format!(
                "{name}: only the {PACKAGE} schema package is allowed"
            ));
        }
        for dependency in &file.dependency {
            if !dependency.starts_with(FILE_PREFIX) {
                violations.push(format!(
                    "{name}: external schema import {dependency} is forbidden"
                ));
            }
        }
        if !file.service.is_empty() {
            violations.push(format!(
                "{name}: peer framing does not use protobuf RPC services"
            ));
        }
        if !file.extension.is_empty() {
            violations.push(format!("{name}: protobuf extensions are forbidden"));
        }
        for message in &file.message_type {
            validate_message(message, PACKAGE, &mut violations);
        }
        for enumeration in &file.enum_type {
            validate_enum(enumeration, PACKAGE, &mut violations);
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "peer v3 protobuf schema policy:\n{}",
            violations.join("\n")
        ))
    }
}

fn validate_message(message: &DescriptorProto, parent: &str, violations: &mut Vec<String>) {
    let name = format!("{parent}.{}", message.name());
    if message
        .options
        .as_ref()
        .is_some_and(|options| options.map_entry())
    {
        violations.push(format!(
            "{name}: maps discard duplicate-key evidence; use repeated entries"
        ));
    }
    if !message.extension.is_empty() || !message.extension_range.is_empty() {
        violations.push(format!("{name}: protobuf extensions are forbidden"));
    }
    for field in &message.field {
        let field_name = format!("{name}.{}", field.name());
        if field.label() == Label::Required {
            violations.push(format!(
                "{field_name}: enforce required values in domain validation"
            ));
        }
        if matches!(field.r#type(), Type::Float | Type::Double | Type::Group) {
            violations.push(format!(
                "{field_name}: floating-point and group fields are forbidden"
            ));
        }
        if matches!(field.r#type(), Type::Message | Type::Enum)
            && !field.type_name().starts_with(TYPE_PREFIX)
        {
            violations.push(format!(
                "{field_name}: external types, Any, and dynamic descriptors are forbidden"
            ));
        }
        // Messages and oneof members already have presence. Repeated fields use
        // their collection semantics; all other scalars must distinguish missing
        // values from an explicitly supplied zero, false, or empty value.
        if field.label() != Label::Repeated
            && field.r#type() != Type::Message
            && field.oneof_index.is_none()
            && !field.proto3_optional()
        {
            violations.push(format!(
                "{field_name}: singular scalar fields must use explicit optional presence"
            ));
        }
    }
    for nested in &message.nested_type {
        validate_message(nested, &name, violations);
    }
    for enumeration in &message.enum_type {
        validate_enum(enumeration, &name, violations);
    }
}

fn validate_enum(enumeration: &EnumDescriptorProto, parent: &str, violations: &mut Vec<String>) {
    let name = format!("{parent}.{}", enumeration.name());
    if !enumeration
        .value
        .first()
        .is_some_and(|value| value.number() == 0 && value.name().ends_with("_UNSPECIFIED"))
    {
        violations.push(format!(
            "{name}: the first enum value must be zero and end in _UNSPECIFIED"
        ));
    }
}
