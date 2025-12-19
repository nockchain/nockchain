#![allow(clippy::unwrap_used)]
use nockvm::mem::NockStack;
use noun_serde::{NounDecode, NounEncode};

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct SingleField {
    x: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct TwoFields {
    x: u64,
    y: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct ThreeFields {
    x: u64,
    y: u64,
    z: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct EmptyStruct;

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct TupleSingle(u64);

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct TupleDouble(u64, u64);

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct TupleTriple(u64, u64, u64);

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct FourFields {
    a: u64,
    b: u64,
    c: u64,
    d: u64,
}

#[derive(Debug, Clone, PartialEq, NounEncode, NounDecode)]
struct FiveFields {
    v: u64,
    w: u64,
    x: u64,
    y: u64,
    z: u64,
}

#[test]
fn test_struct_encoding_no_terminator() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    // Test single field - should encode as just the field value
    let single = SingleField { x: 42 };
    let encoded = single.to_noun(&mut stack);
    // Test that it decodes correctly - this is the actual test
    let decoded = SingleField::from_noun(&encoded).unwrap();
    assert_eq!(single, decoded);
    // Also verify it's a single atom (not wrapped in a cell)
    assert!(
        encoded.as_atom().is_ok(),
        "Single field should encode as an atom"
    );

    // Test two fields - should encode as [x y]
    let two = TwoFields { x: 42, y: 43 };
    let encoded = two.to_noun(&mut stack);
    let decoded = TwoFields::from_noun(&encoded).unwrap();
    assert_eq!(two, decoded);
    // Verify it's a cell with two atoms
    let cell = encoded
        .as_cell()
        .expect("Two fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 42);
    assert_eq!(cell.tail().as_atom().unwrap().as_u64().unwrap(), 43);

    // Test three fields - should encode as [x [y z]]
    let three = ThreeFields {
        x: 42,
        y: 43,
        z: 44,
    };
    let encoded = three.to_noun(&mut stack);
    let decoded = ThreeFields::from_noun(&encoded).unwrap();
    assert_eq!(three, decoded);
    // Verify structure: [42 [43 44]]
    let cell = encoded
        .as_cell()
        .expect("Three fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 42);
    let tail_cell = cell.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail_cell.head().as_atom().unwrap().as_u64().unwrap(), 43);
    assert_eq!(tail_cell.tail().as_atom().unwrap().as_u64().unwrap(), 44);

    // Test empty struct - should encode as 0
    let empty = EmptyStruct;
    let encoded = empty.to_noun(&mut stack);
    let decoded = EmptyStruct::from_noun(&encoded).unwrap();
    assert_eq!(empty, decoded);
    // Verify it's atom 0
    assert_eq!(encoded.as_atom().unwrap().as_u64().unwrap(), 0);

    // Test four fields - should encode as [a [b [c d]]]
    let four = FourFields {
        a: 100,
        b: 101,
        c: 102,
        d: 103,
    };
    let encoded = four.to_noun(&mut stack);
    let decoded = FourFields::from_noun(&encoded).unwrap();
    assert_eq!(four, decoded);
    // Verify structure: [100 [101 [102 103]]]
    let cell = encoded
        .as_cell()
        .expect("Four fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 100);
    let tail1 = cell.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail1.head().as_atom().unwrap().as_u64().unwrap(), 101);
    let tail2 = tail1.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail2.head().as_atom().unwrap().as_u64().unwrap(), 102);
    assert_eq!(tail2.tail().as_atom().unwrap().as_u64().unwrap(), 103);

    // Test five fields - should encode as [v [w [x [y z]]]]
    let five = FiveFields {
        v: 200,
        w: 201,
        x: 202,
        y: 203,
        z: 204,
    };
    let encoded = five.to_noun(&mut stack);
    let decoded = FiveFields::from_noun(&encoded).unwrap();
    assert_eq!(five, decoded);
    // Verify structure: [200 [201 [202 [203 204]]]]
    let cell = encoded
        .as_cell()
        .expect("Five fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 200);
    let tail1 = cell.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail1.head().as_atom().unwrap().as_u64().unwrap(), 201);
    let tail2 = tail1.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail2.head().as_atom().unwrap().as_u64().unwrap(), 202);
    let tail3 = tail2.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail3.head().as_atom().unwrap().as_u64().unwrap(), 203);
    assert_eq!(tail3.tail().as_atom().unwrap().as_u64().unwrap(), 204);
}

#[test]
fn test_tuple_struct_encoding_no_terminator() {
    let mut stack = NockStack::new(8 << 10 << 10, 0);

    // Test single tuple field - should encode as just the field value
    let single = TupleSingle(42);
    let encoded = single.to_noun(&mut stack);
    let decoded = TupleSingle::from_noun(&encoded).unwrap();
    assert_eq!(single, decoded);
    // Verify it's a single atom (not wrapped in a cell)
    assert!(
        encoded.as_atom().is_ok(),
        "Single tuple field should encode as an atom"
    );
    assert_eq!(encoded.as_atom().unwrap().as_u64().unwrap(), 42);

    // Test two tuple fields - should encode as [first second]
    let double = TupleDouble(42, 43);
    let encoded = double.to_noun(&mut stack);
    let decoded = TupleDouble::from_noun(&encoded).unwrap();
    assert_eq!(double, decoded);
    // Verify it's a cell with two atoms
    let cell = encoded
        .as_cell()
        .expect("Two tuple fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 42);
    assert_eq!(cell.tail().as_atom().unwrap().as_u64().unwrap(), 43);

    // Test three tuple fields - should encode as [first [second third]]
    let triple = TupleTriple(42, 43, 44);
    let encoded = triple.to_noun(&mut stack);
    let decoded = TupleTriple::from_noun(&encoded).unwrap();
    assert_eq!(triple, decoded);
    // Verify structure: [42 [43 44]]
    let cell = encoded
        .as_cell()
        .expect("Three tuple fields should encode as a cell");
    assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 42);
    let tail_cell = cell.tail().as_cell().expect("Tail should be a cell");
    assert_eq!(tail_cell.head().as_atom().unwrap().as_u64().unwrap(), 43);
    assert_eq!(tail_cell.tail().as_atom().unwrap().as_u64().unwrap(), 44);
}
