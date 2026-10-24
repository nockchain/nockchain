#![allow(clippy::unwrap_used)]

#[cfg(test)]
use std::ops::{Deref, DerefMut};

#[cfg(test)]
struct TestStackGuard {
    stack: nockvm::mem::NockStack,
}

#[cfg(test)]
impl TestStackGuard {
    fn new(words: usize) -> Self {
        let stack = nockvm::mem::NockStack::new(words, 0);
        Self { stack }
    }
}

#[cfg(test)]
impl Deref for TestStackGuard {
    type Target = nockvm::mem::NockStack;

    fn deref(&self) -> &Self::Target {
        &self.stack
    }
}

#[cfg(test)]
impl DerefMut for TestStackGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stack
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use nockvm::noun::NounSpace;
    use noun_serde::{decode_bool, encode_bool, NounDecode, NounEncode};

    use super::TestStackGuard;

    // Helper struct for testing
    #[derive(Debug, PartialEq, NounEncode, NounDecode)]
    struct TestStruct {
        a: u64,
        b: String,
        c: Option<u64>,
    }

    // Helper enum for testing
    #[derive(Debug, PartialEq, NounEncode, NounDecode)]
    #[noun(tagged = false)]
    enum TestEnum {
        A(u64),
        B { x: String, y: u64 },
        C,
    }

    // Test primitive type encoding/decoding
    #[test]
    fn test_u64_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();
        let original = 42u64;
        let encoded = original.to_noun(&mut *stack);
        let decoded = u64::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_string_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();
        let original = String::from("test");
        let encoded = original.to_noun(&mut *stack);
        let decoded = String::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_option_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Test Some
        let original = Some(42u64);
        let encoded = original.to_noun(&mut *stack);
        let decoded = Option::<u64>::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);

        // Test None
        let original: Option<u64> = None;
        let encoded = original.to_noun(&mut *stack);
        let decoded = Option::<u64>::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_vec_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();
        let original = vec![1u64, 2, 3, 4, 5];
        let encoded = original.to_noun(&mut *stack);
        let decoded = Vec::<u64>::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_bool_encoding() {
        let space = NounSpace::empty();
        // Test true
        let encoded = encode_bool(true);
        let decoded = decode_bool(&encoded, &space).unwrap();
        assert!(decoded);

        // Test false
        let encoded = encode_bool(false);
        let decoded = decode_bool(&encoded, &space).unwrap();
        assert!(!decoded);
    }

    #[test]
    fn test_struct_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();
        let original = TestStruct {
            a: 42,
            b: "test".to_string(),
            c: Some(123),
        };
        let encoded = original.to_noun(&mut *stack);
        println!("encoded TestStruct: {:?}", encoded);
        let decoded = TestStruct::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_enum_variants() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Test variant A (single field)
        let original = TestEnum::A(42);
        println!("\nTesting variant A:");
        println!("original: {:?}", original);
        let encoded = original.to_noun(&mut *stack);
        println!("encoded raw: {:?}", encoded);
        if let Ok(cell) = encoded.as_cell() {
            let cell = cell.in_space(&space);
            println!("head: {:?}", cell.head().noun());
            println!("tail: {:?}", cell.tail().noun());
        }
        let decoded = TestEnum::from_noun(&encoded, &space).unwrap();
        println!("decoded: {:?}", decoded);
        assert_eq!(original, decoded);

        // Test variant B (named fields)
        let original = TestEnum::B {
            x: "test".to_string(),
            y: 123,
        };
        println!("\nTesting variant B:");
        println!("original: {:?}", original);
        let encoded = original.to_noun(&mut *stack);
        println!("encoded raw: {:?}", encoded);
        if let Ok(cell) = encoded.as_cell() {
            let cell = cell.in_space(&space);
            println!("head: {:?}", cell.head().noun());
            println!("tail: {:?}", cell.tail().noun());
            if let Ok(tail_cell) = cell.tail().as_cell() {
                println!("tail.head: {:?}", tail_cell.head().noun());
                println!("tail.tail: {:?}", tail_cell.tail().noun());
            }
        }
        let decoded = TestEnum::from_noun(&encoded, &space).unwrap();
        println!("decoded: {:?}", decoded);
        assert_eq!(original, decoded);

        // Test variant C (unit variant)
        let original = TestEnum::C;
        println!("\nTesting variant C:");
        println!("original: {:?}", original);
        let encoded = original.to_noun(&mut *stack);
        println!("encoded raw: {:?}", encoded);
        if let Ok(cell) = encoded.as_cell() {
            let cell = cell.in_space(&space);
            println!("head: {:?}", cell.head().noun());
            println!("tail: {:?}", cell.tail().noun());
        }
        let decoded = TestEnum::from_noun(&encoded, &space).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_hashset_encoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Create a test set
        let mut set = HashSet::new();
        set.insert(1u64);
        set.insert(2u64);
        set.insert(3u64);

        // Test encoding and decoding
        let encoded = set.to_noun(&mut *stack);
        let decoded = HashSet::<u64>::from_noun(&encoded, &space).unwrap();
        assert_eq!(set, decoded);

        // Test empty set
        let empty_set: HashSet<u64> = HashSet::new();
        let encoded_empty = empty_set.to_noun(&mut *stack);
        let decoded_empty = HashSet::<u64>::from_noun(&encoded_empty, &space).unwrap();
        assert_eq!(empty_set, decoded_empty);
    }

    #[test]
    fn test_nested_option_representation() {
        let mut stack = nockvm::mem::NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let proof = "dummy-proof".to_string();

        let nested = Some(Some(proof));
        let encoded = nested.to_noun(&mut stack);
        let space = stack.noun_space();

        let outer_cell = encoded
            .in_space(&space)
            .as_cell()
            .expect("outer option should be cell");
        assert_eq!(
            outer_cell
                .head()
                .as_atom()
                .expect("outer head should be atom")
                .as_u64()
                .expect("outer head should be 0"),
            0
        );

        let inner_cell = outer_cell
            .tail()
            .as_cell()
            .expect("inner option should be cell");
        assert_eq!(
            inner_cell
                .head()
                .as_atom()
                .expect("inner head should be atom")
                .as_u64()
                .expect("inner head should be 0"),
            0
        );
        assert_eq!(
            String::from_noun(&inner_cell.tail().noun(), &space).expect("decode proof"),
            "dummy-proof"
        );
    }
}

#[cfg(test)]
mod complex_tests {
    use std::collections::HashMap;
    use std::fmt::Debug;

    use nockvm::ext::make_tas;
    use nockvm::noun::{FullDebugCell, Noun, NounAllocator, NounSpace, T};
    use noun_serde::{NounDecode, NounDecodeError, NounEncode};

    use super::TestStackGuard;

    // Complex recursive tree structure
    #[derive(Debug, PartialEq, Clone)]
    enum Tree<T>
    where
        T: NounEncode + NounDecode + Debug + PartialEq + Clone,
    {
        Branch {
            left: Box<Tree<T>>,
            right: Box<Tree<T>>,
            metadata: HashMap<String, Vec<T>>,
        },
        Leaf(T),
    }

    impl<T> NounEncode for Tree<T>
    where
        T: NounEncode + NounDecode + Debug + PartialEq + Clone,
    {
        fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
            match self {
                Tree::Branch {
                    left,
                    right,
                    metadata,
                } => {
                    let tag = make_tas(allocator, "branch").as_noun();
                    let left_noun = left.to_noun(allocator);
                    let right_noun = right.to_noun(allocator);
                    let metadata_noun = metadata.to_noun(allocator);
                    let inner_cell = T(allocator, &[right_noun, metadata_noun]);
                    let data_cell = T(allocator, &[left_noun, inner_cell]);
                    T(allocator, &[tag, data_cell])
                }
                Tree::Leaf(value) => {
                    let tag = make_tas(allocator, "leaf").as_noun();
                    let value_noun = value.to_noun(allocator);
                    T(allocator, &[tag, value_noun])
                }
            }
        }
    }

    impl<T> NounDecode for Tree<T>
    where
        T: NounEncode + NounDecode + Debug + PartialEq + Clone,
    {
        fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
            let cell = noun
                .in_space(space)
                .as_cell()
                .map_err(|_| NounDecodeError::ExpectedCell)?;
            let tag = cell.head().as_atom()?.into_string()?;

            match tag.as_str() {
                "branch" => {
                    let data = cell.tail().as_cell()?;
                    let left_noun = data.head().noun();
                    let left = Box::new(Tree::from_noun(&left_noun, space)?);
                    let rest = data.tail().as_cell()?;
                    let right_noun = rest.head().noun();
                    let right = Box::new(Tree::from_noun(&right_noun, space)?);
                    let metadata_noun = rest.tail().noun();
                    let metadata = HashMap::from_noun(&metadata_noun, space)?;
                    Ok(Tree::Branch {
                        left,
                        right,
                        metadata,
                    })
                }
                "leaf" => {
                    let value_noun = cell.tail().noun();
                    let value = T::from_noun(&value_noun, space)?;
                    Ok(Tree::Leaf(value))
                }
                _ => Err(NounDecodeError::InvalidEnumVariant),
            }
        }
    }

    // Domain model for testing complex structures
    #[derive(Debug, PartialEq, Clone, NounEncode, NounDecode)]
    struct Transaction {
        id: u64,
        timestamp: u64,
        status: TransactionStatus,
        data: TransactionData,
        signatures: Vec<u64>, // Simplified for testing
    }

    #[derive(Debug, PartialEq, Clone, NounEncode, NounDecode)]
    #[noun(tagged = true)]
    enum TransactionStatus {
        Pending { retries: u64, deadline: u64 },
        Complete { result: Result<Vec<u8>, u64> },
        Failed { reason: String, trace: Vec<u64> },
    }

    #[derive(Debug, PartialEq, Clone, NounEncode, NounDecode)]
    struct TransactionData {
        sender: u64,
        receiver: u64,
        amount: u64,
        memo: Option<String>,
        attachments: Vec<u64>, // Simplified for testing
    }

    #[test]
    fn test_recursive_tree() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Create a complex tree
        let mut metadata1 = HashMap::new();
        metadata1.insert("key1".to_string(), vec![1, 2, 3]);

        let mut metadata2 = HashMap::new();
        metadata2.insert("key2".to_string(), vec![4, 5]);

        let tree: Tree<u64> = Tree::Branch {
            left: Box::new(Tree::Branch {
                left: Box::new(Tree::Leaf(1)),
                right: Box::new(Tree::Leaf(2)),
                metadata: metadata1,
            }),
            right: Box::new(Tree::Leaf(3)),
            metadata: metadata2,
        };

        let encoded = tree.to_noun(&mut *stack);
        let encoded_cell = encoded.as_cell().unwrap();
        println!(
            "Encoded tree: {:?}",
            FullDebugCell {
                cell: &encoded_cell,
                space: &space
            }
        );

        let decoded = Tree::from_noun(&encoded, &space).unwrap();
        assert_eq!(tree, decoded);
    }

    #[test]
    fn test_transaction_status() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        let status = TransactionStatus::Pending {
            retries: 3,
            deadline: 99999,
        };

        println!("\nEncoding status: {:?}", status);
        let encoded = status.to_noun(&mut *stack);
        println!("Encoded status noun: {:?}", encoded);

        if let Ok(cell) = encoded.as_cell() {
            let cell = cell.in_space(&space);
            println!("Status cell structure:");
            println!("Head: {:?}", cell.head().noun());
            if let Ok(head_atom) = cell.head().as_atom() {
                if let Ok(tag) = head_atom.into_string() {
                    println!("Tag string: {}", tag);
                }
            }
            println!("Tail: {:?}", cell.tail().noun());
            if let Ok(tail_cell) = cell.tail().as_cell() {
                println!("Tail structure:");
                println!("  Head: {:?}", tail_cell.head().noun());
                println!("  Tail: {:?}", tail_cell.tail().noun());
            }
        }

        println!("\nDecoding status...");
        let decoded = TransactionStatus::from_noun(&encoded, &space).unwrap();
        println!("Decoded status: {:?}", decoded);
        assert_eq!(status, decoded);
    }
    #[test]
    fn test_transaction_data_decoding() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        let original = TransactionData {
            sender: 0x1234,
            receiver: 0x5678,
            amount: 100,
            memo: Some("Test memo".to_string()),
            attachments: vec![1, 2, 3],
        };

        println!("\nEncoding TransactionData: {:?}", original);
        let encoded = original.to_noun(&mut *stack);
        println!("Encoded noun: {:?}", encoded);

        // Print the binary tree structure
        if let Ok(cell) = encoded.as_cell() {
            println!("\nBinary tree structure:");
            println!(
                "Root cell: {:?}",
                FullDebugCell {
                    cell: &cell,
                    space: &space
                }
            );
            let cell_noun = cell.as_noun();
            println!(
                "At axis 2 (sender): {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(2)
                    .map(|handle| handle.noun())
            );
            println!(
                "At axis 3: {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(3)
                    .map(|handle| handle.noun())
            );
            println!(
                "At axis 6 (receiver): {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(6)
                    .map(|handle| handle.noun())
            );
            println!(
                "At axis 7: {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(7)
                    .map(|handle| handle.noun())
            );
            println!(
                "At axis 14 (amount): {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(14)
                    .map(|handle| handle.noun())
            );
            println!(
                "At axis 15: {:?}",
                cell_noun
                    .in_space(&space)
                    .slot(15)
                    .map(|handle| handle.noun())
            );
        }

        println!("\nDecoding TransactionData...");
        let decoded = TransactionData::from_noun(&encoded, &space).unwrap();
        println!("Decoded result: {:?}", decoded);

        assert_eq!(original, decoded);
    }
    #[test]
    fn test_complex_transaction() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        let transaction = Transaction {
            id: 1,
            timestamp: 12345,
            status: TransactionStatus::Pending {
                retries: 3,
                deadline: 99999,
            },
            data: TransactionData {
                sender: 0x1234,
                receiver: 0x5678,
                amount: 100,
                memo: Some("Test transaction".to_string()),
                attachments: vec![1, 2, 3],
            },
            signatures: vec![0xdead, 0xbeef],
        };

        println!("\nEncoding transaction: {:?}", transaction);
        let encoded = transaction.to_noun(&mut *stack);
        println!("\nEncoded transaction noun: {:?}", encoded);
        let encoded_handle = encoded.in_space(&space);
        if let Ok(cell) = encoded_handle.as_cell() {
            println!("Transaction cell head: {:?}", cell.head().noun());
            println!("Transaction cell tail: {:?}", cell.tail().noun());
            if let Ok(status_cell) = encoded_handle.slot(7).and_then(|h| h.as_cell()) {
                println!("Status tag: {:?}", status_cell.head().noun());
                println!("Status data: {:?}", status_cell.tail().noun());
            }
        }

        println!("\nDecoding transaction...");
        let decoded = Transaction::from_noun(&encoded, &space).unwrap();
        println!("Successfully decoded transaction: {:?}", decoded);
        assert_eq!(transaction, decoded);

        // Test different status variants
        let mut transaction2 = transaction.clone();
        transaction2.status = TransactionStatus::Complete {
            result: Ok(vec![1, 2, 3]),
        };
        let encoded2 = transaction2.to_noun(&mut *stack);
        let decoded2 = Transaction::from_noun(&encoded2, &space).unwrap();
        assert_eq!(transaction2, decoded2);

        let mut transaction3 = transaction;
        transaction3.status = TransactionStatus::Failed {
            reason: "Test failure".to_string(),
            trace: vec![404, 500],
        };
        let encoded3 = transaction3.to_noun(&mut *stack);
        let decoded3 = Transaction::from_noun(&encoded3, &space).unwrap();
        assert_eq!(transaction3, decoded3);
    }

    #[test]
    fn test_nested_options_and_results() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Test deeply nested Option<Result<Option<T>>>
        let nested_data: Option<Result<Option<Vec<u64>>, String>> = Some(Ok(Some(vec![1, 2, 3])));

        let encoded = nested_data.to_noun(&mut *stack);
        let encoded_cell = encoded.as_cell().unwrap();
        println!(
            "Encoded nested data: {:?}",
            FullDebugCell {
                cell: &encoded_cell,
                space: &space
            }
        );

        let decoded =
            Option::<Result<Option<Vec<u64>>, String>>::from_noun(&encoded, &space).unwrap();
        assert_eq!(nested_data, decoded);

        // Test None case
        let none_data: Option<Result<Option<Vec<u64>>, String>> = None;
        let encoded_none = none_data.to_noun(&mut *stack);
        let decoded_none =
            Option::<Result<Option<Vec<u64>>, String>>::from_noun(&encoded_none, &space).unwrap();
        assert_eq!(none_data, decoded_none);

        // Test Error case
        let err_data: Option<Result<Option<Vec<u64>>, String>> =
            Some(Err("test error".to_string()));
        let encoded_err = err_data.to_noun(&mut *stack);
        let decoded_err =
            Option::<Result<Option<Vec<u64>>, String>>::from_noun(&encoded_err, &space).unwrap();
        assert_eq!(err_data, decoded_err);
    }

    #[test]
    fn test_complex_collections() {
        let mut stack = TestStackGuard::new(8 << 10 << 10);
        let space = stack.noun_space();

        // Test Vec<HashMap<String, Vec<Option<u64>>>>
        let mut map1 = HashMap::new();
        map1.insert("key1".to_string(), vec![Some(1), None, Some(3)]);
        map1.insert("key2".to_string(), vec![Some(4), Some(5)]);

        let mut map2 = HashMap::new();
        map2.insert("key3".to_string(), vec![None, None]);

        let complex_collection = vec![map1, map2];

        let encoded = complex_collection.to_noun(&mut *stack);
        let encoded_cell = encoded.as_cell().unwrap();
        println!(
            "Encoded collection: {:?}",
            FullDebugCell {
                cell: &encoded_cell,
                space: &space
            }
        );

        let decoded =
            Vec::<HashMap<String, Vec<Option<u64>>>>::from_noun(&encoded, &space).unwrap();
        assert_eq!(complex_collection, decoded);
    }
}
