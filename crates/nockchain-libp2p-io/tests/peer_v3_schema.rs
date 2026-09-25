//! Schema regression tests run against the same descriptors used for codegen.

use prost::Message;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FileDescriptorProto, FileDescriptorSet, MessageOptions, OneofDescriptorProto,
};

#[path = "../build_support/schema.rs"]
mod schema;

fn descriptor() -> FileDescriptorSet {
    FileDescriptorSet::decode(
        include_bytes!(concat!(env!("OUT_DIR"), "/peer_v3_descriptor.bin")).as_slice(),
    )
    .expect("the build must produce a protobuf descriptor")
}

fn fixture(field: FieldDescriptorProto) -> FileDescriptorSet {
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("nockchain/peer/v3/fixture.proto".to_owned()),
            package: Some("nockchain.peer.v3".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Fixture".to_owned()),
                field: vec![field],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn field(field_type: Type) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some("value".to_owned()),
        number: Some(1),
        label: Some(Label::Optional as i32),
        r#type: Some(field_type as i32),
        proto3_optional: Some(true),
        ..Default::default()
    }
}

#[test]
fn production_schemas_satisfy_the_peer_profile() {
    schema::validate_peer_schema(&descriptor()).expect("peer schema policy");
}

#[test]
fn adding_an_implicitly_present_scalar_is_a_build_error() {
    let mut changed = descriptor();
    changed.file[0].message_type.push(DescriptorProto {
        name: Some("FutureMessage".to_owned()),
        field: vec![FieldDescriptorProto {
            proto3_optional: Some(false),
            ..field(Type::Uint64)
        }],
        ..Default::default()
    });
    let error = schema::validate_peer_schema(&changed).expect_err("implicit scalar presence");
    assert!(error.contains("FutureMessage.value"));
    assert!(error.contains("explicit optional presence"));
}

#[test]
fn typed_message_and_oneof_members_already_have_presence() {
    let mut message_field = field(Type::Message);
    message_field.proto3_optional = Some(false);
    message_field.type_name = Some(".nockchain.peer.v3.Fixture".to_owned());
    schema::validate_peer_schema(&fixture(message_field)).expect("message presence");

    let mut alternative = field(Type::Uint64);
    alternative.proto3_optional = Some(false);
    alternative.oneof_index = Some(0);
    let mut oneof = fixture(alternative);
    oneof.file[0].message_type[0]
        .oneof_decl
        .push(OneofDescriptorProto {
            name: Some("selection".to_owned()),
            ..Default::default()
        });
    schema::validate_peer_schema(&oneof).expect("oneof presence");

    let mut repeated = field(Type::Uint64);
    repeated.proto3_optional = Some(false);
    repeated.label = Some(Label::Repeated as i32);
    schema::validate_peer_schema(&fixture(repeated)).expect("collection semantics");
}

#[test]
fn nested_maps_cannot_bypass_the_profile() {
    let mut changed = fixture(field(Type::Uint64));
    changed.file[0].message_type[0]
        .nested_type
        .push(DescriptorProto {
            name: Some("NestedMapEntry".to_owned()),
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        });
    let error = schema::validate_peer_schema(&changed).expect_err("map entry");
    assert!(error.contains("Fixture.NestedMapEntry"));
    assert!(error.contains("duplicate-key evidence"));
}

#[test]
fn peer_schemas_cannot_import_any_or_dynamic_descriptors() {
    for external in [".google.protobuf.Any", ".google.protobuf.FileDescriptorSet"] {
        let mut external_field = field(Type::Message);
        external_field.type_name = Some(external.to_owned());
        let error = schema::validate_peer_schema(&fixture(external_field))
            .expect_err("external message type");
        assert!(error.contains("external types"));
    }
    let mut changed = fixture(field(Type::Uint64));
    changed.file[0]
        .dependency
        .push("google/protobuf/any.proto".to_owned());
    assert!(schema::validate_peer_schema(&changed)
        .expect_err("external import")
        .contains("external schema import"));
}

#[test]
fn ambiguous_numeric_and_legacy_field_types_are_rejected() {
    for field_type in [Type::Float, Type::Double, Type::Group] {
        assert!(schema::validate_peer_schema(&fixture(field(field_type)))
            .expect_err("forbidden field type")
            .contains("floating-point and group fields"));
    }
    let mut required = field(Type::Uint64);
    required.label = Some(Label::Required as i32);
    assert!(schema::validate_peer_schema(&fixture(required))
        .expect_err("legacy required field")
        .contains("enforce required values in domain validation"));
}

#[test]
fn default_enum_value_cannot_select_an_operation() {
    let mut changed = fixture(field(Type::Uint64));
    changed.file[0].message_type[0]
        .enum_type
        .push(EnumDescriptorProto {
            name: Some("Operation".to_owned()),
            value: vec![EnumValueDescriptorProto {
                name: Some("OPERATION_APPLY".to_owned()),
                number: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        });
    assert!(schema::validate_peer_schema(&changed)
        .expect_err("zero selects an operation")
        .contains("_UNSPECIFIED"));
    changed.file[0].message_type[0].enum_type[0].value[0].name =
        Some("OPERATION_UNSPECIFIED".to_owned());
    schema::validate_peer_schema(&changed).expect("unspecified zero enum value");
}
