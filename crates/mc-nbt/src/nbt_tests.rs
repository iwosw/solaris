use super::*;
use proptest::prelude::*;

fn read_string<B: Buf>(buf: &mut B) -> Result<String, NbtError> {
    let mut budget = NbtBudget::production();
    read_string_with_budget(buf, &mut budget)
}

fn round_trip_network(tag: Tag) {
    let mut buf = Vec::new();
    write_network(&mut buf, &tag).unwrap();
    let mut cursor: &[u8] = &buf;
    let decoded = read_network(&mut cursor).unwrap();
    assert_eq!(decoded, tag);
    assert!(cursor.is_empty(), "all bytes consumed");
}

fn round_trip_named(name: &str, tag: Tag) {
    let mut buf = Vec::new();
    write_named(&mut buf, name, &tag).unwrap();
    let mut cursor: &[u8] = &buf;
    let (decoded_name, decoded) = read_named(&mut cursor).unwrap();
    assert_eq!(decoded_name, name);
    assert_eq!(decoded, tag);
    assert!(cursor.is_empty());
}

fn read_network_with_limits(bytes: &[u8], limits: NbtLimits) -> Result<Tag, NbtError> {
    let mut cursor = bytes;
    let mut budget = NbtBudget::new(limits);
    read_network_with_budget(&mut cursor, &mut budget)
}

fn validate_network_with_limits(root: &Tag, limits: NbtLimits) -> Result<(), NbtError> {
    let mut budget = NbtBudget::new(limits);
    budget.charge_wire(1)?;
    validate_payload(root, 1, &mut budget)
}

#[test]
fn empty_compound_round_trip() {
    round_trip_network(Tag::Compound(vec![]));
    round_trip_named("", Tag::Compound(vec![]));
    round_trip_named("hello", Tag::Compound(vec![]));
}

#[test]
fn many_small_compounds_fit_unchanged_allocation_budget() {
    // Canonical bastion air_base contains 23,447 tiny block compounds in an
    // 845 KiB document. Reserving 64 entries for every three-field record
    // exhausts the 64 MiB budget despite the small actual content.
    let root = Tag::Compound(vec![(
        "blocks".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: (0..24_000)
                .map(|index| {
                    Tag::Compound(vec![
                        ("x".into(), Tag::Int(index)),
                        ("y".into(), Tag::Int(0)),
                        ("z".into(), Tag::Int(0)),
                    ])
                })
                .collect(),
        }),
    )]);
    let mut bytes = Vec::new();
    write_named(&mut bytes, "", &root).unwrap();
    assert!(bytes.len() < 1024 * 1024);
    let mut cursor = bytes.as_slice();
    let mut budget = NbtBudget::production();
    let (_, decoded) = read_named_with_budget(&mut cursor, &mut budget).unwrap();
    assert_eq!(decoded, root);
    assert!(cursor.is_empty());
    assert!(budget.allocation_bytes < MAX_NBT_ALLOCATION_BYTES / 4);
    let mut document = Vec::new();
    write_named_document(&mut document, "", &root).unwrap();
    assert_eq!(
        document, bytes,
        "checkpoint preflight models the same capacity policy"
    );
}

#[test]
fn primitives_round_trip() {
    round_trip_network(Tag::Compound(vec![
        ("b".into(), Tag::Byte(-1)),
        ("s".into(), Tag::Short(-32_000)),
        ("i".into(), Tag::Int(0x7FFF_FFFF)),
        ("l".into(), Tag::Long(i64::MIN)),
        ("f".into(), Tag::Float(core::f32::consts::PI)),
        ("d".into(), Tag::Double(core::f64::consts::E)),
    ]));
}

#[test]
fn strings_round_trip() {
    round_trip_network(Tag::Compound(vec![
        ("ascii".into(), Tag::String("hello, world".into())),
        ("bmp".into(), Tag::String("ümlauts, шифры, 漢字".into())),
        ("modified".into(), Tag::String("nul:\0 face:😀".into())),
        ("empty".into(), Tag::String(String::new())),
    ]));
}

#[test]
fn modified_utf8_encodes_ascii_exactly() {
    let mut buf = Vec::new();
    write_string(&mut buf, "ASCII").unwrap();

    assert_eq!(buf, [0x00, 0x05, b'A', b'S', b'C', b'I', b'I']);

    let mut cursor = buf.as_slice();
    assert_eq!(read_string(&mut cursor).unwrap(), "ASCII");
    assert!(cursor.is_empty());
}

#[test]
fn modified_utf8_encodes_embedded_nul_as_c0_80() {
    let mut buf = Vec::new();
    write_string(&mut buf, "A\0B").unwrap();

    assert_eq!(buf, [0x00, 0x04, b'A', 0xC0, 0x80, b'B']);

    let mut cursor = buf.as_slice();
    assert_eq!(read_string(&mut cursor).unwrap(), "A\0B");
    assert!(cursor.is_empty());
}

#[test]
fn modified_utf8_encodes_bmp_boundaries_exactly() {
    let mut buf = Vec::new();
    write_string(&mut buf, "\u{0001}\u{007f}\u{0080}\u{07ff}\u{0800}\u{ffff}").unwrap();

    assert_eq!(
        buf,
        vec![
            0x00, 0x0C, 0x01, 0x7F, 0xC2, 0x80, 0xDF, 0xBF, 0xE0, 0xA0, 0x80, 0xEF, 0xBF, 0xBF,
        ]
    );

    let mut cursor = buf.as_slice();
    assert_eq!(
        read_string(&mut cursor).unwrap(),
        "\u{0001}\u{007f}\u{0080}\u{07ff}\u{0800}\u{ffff}"
    );
    assert!(cursor.is_empty());
}

#[test]
fn modified_utf8_encodes_supplementary_code_point_as_surrogate_pair() {
    let mut buf = Vec::new();
    write_string(&mut buf, "😀").unwrap();

    assert_eq!(buf, [0x00, 0x06, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);

    let mut cursor = buf.as_slice();
    assert_eq!(read_string(&mut cursor).unwrap(), "😀");
    assert!(cursor.is_empty());
}

#[test]
fn modified_utf8_rejects_standard_four_byte_sequence() {
    let mut cursor: &[u8] = &[0x00, 0x04, 0xF0, 0x9F, 0x98, 0x80];
    assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
}

#[test]
fn modified_utf8_rejects_noncanonical_and_truncated_sequences() {
    for bytes in [
        &[0x00][..],
        &[0x80],
        &[0xC1, 0x81],
        &[0xC2],
        &[0xC2, b'A'],
        &[0xE0, 0x80, 0x80],
        &[0xE1, 0x80],
        &[0xE1, b'A', 0x80],
    ] {
        let mut encoded = Vec::with_capacity(bytes.len() + 2);
        encoded.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        encoded.extend_from_slice(bytes);
        let mut cursor = encoded.as_slice();
        assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
    }

    let mut truncated_payload: &[u8] = &[0x00, 0x02, 0xC2];
    assert_eq!(
        read_string(&mut truncated_payload),
        Err(NbtError::Underflow {
            needed: 1,
            available: 1,
        })
    );
}

#[test]
fn modified_utf8_rejects_unpaired_and_reversed_surrogates() {
    for bytes in [
        &[0xED, 0xA0, 0x80][..],
        &[0xED, 0xB0, 0x80],
        &[0xED, 0xB0, 0x80, 0xED, 0xA0, 0x80],
        &[0xED, 0xA0, 0x80, 0xED, 0xA0, 0x80],
    ] {
        let mut encoded = Vec::with_capacity(bytes.len() + 2);
        encoded.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        encoded.extend_from_slice(bytes);
        let mut cursor = encoded.as_slice();
        assert_eq!(read_string(&mut cursor), Err(NbtError::InvalidString));
    }
}

#[test]
fn modified_utf8_enforces_encoded_u16_wire_length() {
    let exact_limit = "a".repeat(usize::from(u16::MAX));
    let mut output = Vec::new();
    write_string(&mut output, &exact_limit).unwrap();
    assert_eq!(&output[..2], &[0xFF, 0xFF]);
    assert_eq!(output.len(), usize::from(u16::MAX) + 2);
    let mut cursor = output.as_slice();
    assert_eq!(read_string(&mut cursor).unwrap(), exact_limit);
    assert!(cursor.is_empty());

    let too_long = "\0".repeat(32_768);
    output.clear();
    assert_eq!(
        write_string(&mut output, &too_long),
        Err(NbtError::StringTooLong(65_536))
    );
    assert!(output.is_empty());
}

#[test]
fn arrays_round_trip() {
    round_trip_network(Tag::Compound(vec![
        (
            "ba".into(),
            Tag::ByteArray((0..50).map(|i| i as i8).collect()),
        ),
        (
            "ia".into(),
            Tag::IntArray((0..30).map(|i| i * 1_000_000 - 15_000_000).collect()),
        ),
        (
            "la".into(),
            // Span both signs, cover MIN/MAX, but stay inside i64.
            Tag::LongArray(vec![
                0,
                1,
                -1,
                i64::MIN,
                i64::MAX,
                i64::MAX / 19,
                -(i64::MAX / 7),
            ]),
        ),
    ]));
}

#[test]
fn list_round_trip() {
    round_trip_network(Tag::Compound(vec![
        (
            "ints".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
            }),
        ),
        ("empty".into(), Tag::List(ListTag::empty())),
        (
            "nested_compounds".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![
                    Tag::Compound(vec![("k".into(), Tag::Byte(1))]),
                    Tag::Compound(vec![("k".into(), Tag::Byte(2))]),
                ],
            }),
        ),
    ]));
}

#[test]
fn nested_lists_and_empty_unknown_element_type_round_trip() {
    round_trip_network(Tag::Compound(vec![
        (
            "nested".into(),
            Tag::List(ListTag {
                element_type: tag_type::LIST,
                elements: vec![
                    Tag::List(ListTag {
                        element_type: tag_type::INT,
                        elements: vec![Tag::Int(1)],
                    }),
                    Tag::List(ListTag {
                        element_type: tag_type::INT,
                        elements: vec![Tag::Int(2), Tag::Int(3)],
                    }),
                ],
            }),
        ),
        (
            "empty_unknown".into(),
            Tag::List(ListTag {
                element_type: 0x7f,
                elements: Vec::new(),
            }),
        ),
    ]));
}

#[test]
fn non_empty_unknown_list_element_type_is_rejected() {
    let mut encoded = vec![tag_type::COMPOUND, tag_type::LIST, 0, 0, 0x7f];
    encoded.extend_from_slice(&1_i32.to_be_bytes());
    encoded.push(0);

    assert_eq!(
        read_network(&mut encoded.as_slice()),
        Err(NbtError::UnknownTag(0x7f))
    );
}

#[test]
fn truncated_list_fails_feasibility_before_element_allocation() {
    let mut encoded = vec![tag_type::COMPOUND, tag_type::LIST, 0, 0, tag_type::INT];
    encoded.extend_from_slice(&2_i32.to_be_bytes());
    encoded.extend_from_slice(&1_i32.to_be_bytes());

    assert_eq!(
        read_network(&mut encoded.as_slice()),
        Err(NbtError::Underflow {
            needed: 4,
            available: 4,
        })
    );
}

#[test]
fn duplicate_compound_names_round_trip_in_order() {
    round_trip_network(Tag::Compound(vec![
        ("same".into(), Tag::Byte(1)),
        ("same".into(), Tag::Byte(2)),
    ]));
}

#[test]
fn reader_enforces_aggregate_wire_node_entry_string_and_allocation_budgets() {
    let simple = Tag::Compound(vec![("x".into(), Tag::Byte(1))]);
    let mut simple_bytes = Vec::new();
    write_network(&mut simple_bytes, &simple).unwrap();
    let wire_limit = simple_bytes.len() - 1;
    assert_eq!(
        read_network_with_limits(
            &simple_bytes,
            NbtLimits {
                wire_bytes: wire_limit,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "wire-byte",
            limit: wire_limit,
        })
    );

    let list = Tag::Compound(vec![(
        "list".into(),
        Tag::List(ListTag {
            element_type: tag_type::INT,
            elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
        }),
    )]);
    let mut list_bytes = Vec::new();
    write_network(&mut list_bytes, &list).unwrap();
    assert_eq!(
        read_network_with_limits(
            &list_bytes,
            NbtLimits {
                nodes: 4,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "node",
            limit: 4,
        })
    );

    let entries = Tag::Compound(vec![("a".into(), Tag::Byte(1)), ("b".into(), Tag::Byte(2))]);
    let mut entry_bytes = Vec::new();
    write_network(&mut entry_bytes, &entries).unwrap();
    assert_eq!(
        read_network_with_limits(
            &entry_bytes,
            NbtLimits {
                compound_entries: 1,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "compound-entry",
            limit: 1,
        })
    );

    let strings = Tag::Compound(vec![("name".into(), Tag::String("value".into()))]);
    let mut string_bytes = Vec::new();
    write_network(&mut string_bytes, &strings).unwrap();
    assert_eq!(
        read_network_with_limits(
            &string_bytes,
            NbtLimits {
                string_bytes: 8,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "string-byte",
            limit: 8,
        })
    );

    let mut baseline_budget = NbtBudget::production();
    let mut baseline_cursor = list_bytes.as_slice();
    read_network_with_budget(&mut baseline_cursor, &mut baseline_budget).unwrap();
    let allocation_limit = baseline_budget.allocation_bytes - 1;
    assert_eq!(
        read_network_with_limits(
            &list_bytes,
            NbtLimits {
                allocation_bytes: allocation_limit,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            limit: allocation_limit,
        })
    );
    assert!(
        read_network_with_limits(
            &list_bytes,
            NbtLimits {
                allocation_bytes: baseline_budget.allocation_bytes,
                ..PRODUCTION_LIMITS
            },
        )
        .is_ok()
    );
}

#[test]
fn writer_preflight_enforces_aggregate_budgets() {
    let root = Tag::Compound(vec![
        (
            "list".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)],
            }),
        ),
        ("value".into(), Tag::String("payload".into())),
    ]);

    assert_eq!(
        validate_network_with_limits(
            &root,
            NbtLimits {
                nodes: 5,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "node",
            limit: 5,
        })
    );
    assert_eq!(
        validate_network_with_limits(
            &root,
            NbtLimits {
                compound_entries: 1,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "compound-entry",
            limit: 1,
        })
    );
    assert_eq!(
        validate_network_with_limits(
            &root,
            NbtLimits {
                string_bytes: 15,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "string-byte",
            limit: 15,
        })
    );
    assert_eq!(
        validate_network_with_limits(
            &root,
            NbtLimits {
                wire_bytes: 24,
                ..PRODUCTION_LIMITS
            },
        ),
        Err(NbtError::BudgetExceeded {
            resource: "wire-byte",
            limit: 24,
        })
    );
}

#[test]
fn reader_and_writer_enforce_per_compound_entry_limit() {
    let mut encoded = vec![tag_type::COMPOUND];
    for _ in 0..=MAX_NBT_COMPOUND_ENTRIES {
        encoded.extend_from_slice(&[tag_type::BYTE, 0, 0, 0]);
    }
    encoded.push(tag_type::END);
    assert_eq!(
        read_network(&mut encoded.as_slice()),
        Err(NbtError::BudgetExceeded {
            resource: "compound-entry-per-container",
            limit: MAX_NBT_COMPOUND_ENTRIES,
        })
    );

    let root = Tag::Compound(
        (0..=MAX_NBT_COMPOUND_ENTRIES)
            .map(|_| (String::new(), Tag::Byte(0)))
            .collect(),
    );
    let mut output = vec![0xA5];
    assert_eq!(
        write_network(&mut output, &root),
        Err(NbtError::BudgetExceeded {
            resource: "compound-entry-per-container",
            limit: MAX_NBT_COMPOUND_ENTRIES,
        })
    );
    assert_eq!(output, vec![0xA5]);
}

#[test]
fn compound_capacity_growth_is_charged_before_reserve() {
    let mut budget = NbtBudget::production();
    let mut entries = Vec::new();
    ensure_budgeted_compound_capacity(&mut entries, &mut budget).unwrap();
    let initial_capacity = entries.capacity();
    assert!(initial_capacity >= COMPOUND_RESERVE_CHUNK);
    while entries.len() < initial_capacity {
        entries.push((String::new(), Tag::Byte(0)));
    }

    budget.limits.allocation_bytes = budget.allocation_bytes;
    let before_capacity = entries.capacity();
    assert_eq!(
        ensure_budgeted_compound_capacity(&mut entries, &mut budget),
        Err(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            limit: budget.allocation_bytes,
        })
    );
    assert_eq!(entries.capacity(), before_capacity);
}

#[test]
fn compound_capacity_growth_charges_only_final_capacity() {
    let mut budget = NbtBudget::production();
    let mut entries = Vec::new();

    for _ in 0..4 {
        ensure_budgeted_compound_capacity(&mut entries, &mut budget).unwrap();
        let capacity = entries.capacity();
        while entries.len() < capacity {
            entries.push((String::new(), Tag::Byte(0)));
        }
    }

    assert_eq!(
        budget.allocation_bytes,
        capacity_bytes::<(String, Tag)>(entries.capacity(), budget.limits.allocation_bytes)
            .unwrap()
    );
}

#[test]
fn deeply_nested_compound() {
    let leaf = Tag::String("hi".into());
    let mut tag = Tag::Compound(vec![("leaf".into(), leaf)]);
    for i in 0..32 {
        tag = Tag::Compound(vec![(format!("level{i}"), tag)]);
    }
    round_trip_network(tag);
}

#[test]
fn write_rejects_heterogeneous_list() {
    let bad = Tag::Compound(vec![(
        "k".into(),
        Tag::List(ListTag {
            element_type: tag_type::INT,
            elements: vec![Tag::Int(1), Tag::Byte(2)],
        }),
    )]);
    let mut buf = Vec::new();
    let err = write_network(&mut buf, &bad).unwrap_err();
    assert!(matches!(err, NbtError::HeterogeneousList { .. }));
}

#[test]
fn writer_errors_leave_existing_output_unchanged() {
    let heterogeneous = Tag::Compound(vec![(
        "k".into(),
        Tag::List(ListTag {
            element_type: tag_type::INT,
            elements: vec![Tag::Int(1), Tag::Byte(2)],
        }),
    )]);
    let mut network_output = vec![0xA5, 0x5A];
    assert!(matches!(
        write_network(&mut network_output, &heterogeneous),
        Err(NbtError::HeterogeneousList { .. })
    ));
    assert_eq!(network_output, vec![0xA5, 0x5A]);

    let long_name = "x".repeat(usize::from(u16::MAX) + 1);
    let mut named_output = vec![0x11, 0x22];
    assert!(matches!(
        write_named(&mut named_output, &long_name, &Tag::Compound(Vec::new())),
        Err(NbtError::StringTooLong(_))
    ));
    assert_eq!(named_output, vec![0x11, 0x22]);
}

#[test]
fn document_preflight_rejects_compound_allocation_expansion_atomically() {
    let root = Tag::Compound(vec![(
        "records".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: (0..20_000)
                .map(|_| Tag::Compound(vec![(String::new(), Tag::Byte(1))]))
                .collect(),
        }),
    )]);
    let mut output = vec![0x11, 0x22];
    assert!(matches!(
        write_named_document(&mut output, "", &root),
        Err(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            ..
        })
    ));
    assert_eq!(output, vec![0x11, 0x22]);

    // Existing generic writing stays unchanged. The document policy additionally
    // refuses trees that its bounded decoder cannot allocate on restart.
    let mut encoded = Vec::new();
    write_named(&mut encoded, "", &root).unwrap();
    assert!(matches!(
        read_named_document(&mut encoded.as_slice()),
        Err(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            ..
        })
    ));
}

#[test]
fn write_rejects_non_compound_root() {
    let mut buf = Vec::new();
    let err = write_network(&mut buf, &Tag::Int(42)).unwrap_err();
    assert!(matches!(err, NbtError::RootMustBeCompound(_)));
}

#[test]
fn read_rejects_non_compound_root() {
    let buf: &[u8] = &[tag_type::INT, 0, 0, 0, 0]; // tag-id 3, four payload bytes
    let mut cursor = buf;
    let err = read_network(&mut cursor).unwrap_err();
    assert!(matches!(err, NbtError::NotCompound(tag_type::INT)));
}

#[test]
fn read_rejects_negative_length() {
    // type=Compound, then BYTE_ARRAY field "x" with length=-1
    let mut buf: Vec<u8> = vec![tag_type::COMPOUND, tag_type::BYTE_ARRAY, 0, 1, b'x'];
    buf.extend_from_slice(&(-1i32).to_be_bytes());
    let mut cursor: &[u8] = &buf;
    let err = read_network(&mut cursor).unwrap_err();
    assert!(matches!(err, NbtError::NegativeLength(-1)));
}

#[test]
fn read_rejects_oversized_length() {
    // BYTE_ARRAY field with length just over the ceiling.
    let mut buf: Vec<u8> = vec![tag_type::COMPOUND, tag_type::BYTE_ARRAY, 0, 1, b'x'];
    buf.extend_from_slice(&((MAX_NBT_LENGTH as i32 + 1).to_be_bytes()));
    let mut cursor: &[u8] = &buf;
    let err = read_network(&mut cursor).unwrap_err();
    assert!(matches!(err, NbtError::ArrayTooLong(_)));
}

#[test]
fn int_array_limit_counts_payload_bytes() {
    let too_many_ints = MAX_NBT_LENGTH / size_of::<i32>() + 1;
    let mut encoded = vec![tag_type::COMPOUND, tag_type::INT_ARRAY, 0, 1, b'x'];
    encoded.extend_from_slice(&(too_many_ints as i32).to_be_bytes());
    let mut cursor = encoded.as_slice();

    assert!(matches!(
        read_network(&mut cursor),
        Err(NbtError::ArrayTooLong(_))
    ));

    let tag = Tag::Compound(vec![("x".into(), Tag::IntArray(vec![0; too_many_ints]))]);
    let mut output = Vec::new();
    assert!(matches!(
        write_network(&mut output, &tag),
        Err(NbtError::ArrayTooLong(_))
    ));
}

#[test]
fn reader_and_writer_accept_exact_depth_boundary() {
    let nested_containers = MAX_NBT_DEPTH - 1;
    let mut tag = Tag::Compound(Vec::new());
    for _ in 0..nested_containers {
        tag = Tag::Compound(vec![(String::new(), tag)]);
    }
    round_trip_network(tag);
}

#[test]
fn reader_and_writer_reject_excessive_nesting() {
    let nested_containers = MAX_NBT_DEPTH;
    let mut encoded = vec![tag_type::COMPOUND];
    for _ in 0..nested_containers {
        encoded.extend_from_slice(&[tag_type::COMPOUND, 0, 0]);
    }
    encoded.resize(encoded.len() + nested_containers + 1, tag_type::END);
    let mut cursor = encoded.as_slice();
    assert_eq!(read_network(&mut cursor), Err(NbtError::NestingTooDeep));

    let mut tag = Tag::Compound(Vec::new());
    for _ in 0..nested_containers {
        tag = Tag::Compound(vec![(String::new(), tag)]);
    }
    let mut output = Vec::new();
    assert_eq!(
        write_network(&mut output, &tag),
        Err(NbtError::NestingTooDeep)
    );
}

#[test]
fn read_rejects_truncated_buffer() {
    // Compound header but no payload bytes follow.
    let buf: &[u8] = &[tag_type::COMPOUND, tag_type::INT, 0, 1, b'x'];
    let mut cursor = buf;
    assert!(matches!(
        read_network(&mut cursor),
        Err(NbtError::Underflow { .. })
    ));
}

// ---- Property tests ---------------------------------------------------

fn arb_simple_tag() -> impl Strategy<Value = Tag> {
    prop_oneof![
        any::<i8>().prop_map(Tag::Byte),
        any::<i16>().prop_map(Tag::Short),
        any::<i32>().prop_map(Tag::Int),
        any::<i64>().prop_map(Tag::Long),
        // Use `from_bits` to also exercise NaN/infinity values.
        any::<u32>().prop_map(|b| Tag::Float(f32::from_bits(b))),
        any::<u64>().prop_map(|b| Tag::Double(f64::from_bits(b))),
        "[\\x20-\\x7E]{0,128}".prop_map(Tag::String),
        proptest::collection::vec(any::<i8>(), 0..256).prop_map(Tag::ByteArray),
        proptest::collection::vec(any::<i32>(), 0..128).prop_map(Tag::IntArray),
        proptest::collection::vec(any::<i64>(), 0..128).prop_map(Tag::LongArray),
    ]
}

fn arb_compound() -> impl Strategy<Value = Tag> {
    proptest::collection::vec(("[a-z][a-z_0-9]{0,16}", arb_simple_tag()), 0..16)
        .prop_map(Tag::Compound)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn proptest_compound_round_trip(tag in arb_compound()) {
        // Re-implement round_trip_network inline so we can compare with
        // bitwise equality for floats (NaN != NaN otherwise).
        let mut buf = Vec::new();
        write_network(&mut buf, &tag).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded = read_network(&mut cursor).unwrap();
        prop_assert!(tags_bitwise_eq(&decoded, &tag));
        prop_assert!(cursor.is_empty());
    }

    #[test]
    fn proptest_named_round_trip(
        name in "[a-z]{0,32}",
        tag in arb_compound(),
    ) {
        let mut buf = Vec::new();
        write_named(&mut buf, &name, &tag).unwrap();
        let mut cursor: &[u8] = &buf;
        let (decoded_name, decoded) = read_named(&mut cursor).unwrap();
        prop_assert_eq!(decoded_name, name);
        prop_assert!(tags_bitwise_eq(&decoded, &tag));
    }

    #[test]
    fn proptest_modified_utf8_round_trip(value in any::<String>()) {
        let mut buf = Vec::new();
        write_string(&mut buf, &value).unwrap();
        let mut cursor = buf.as_slice();
        let decoded = read_string(&mut cursor).unwrap();
        prop_assert_eq!(decoded, value);
        prop_assert!(cursor.is_empty());
    }
}

/// Compare tags with bitwise float equality so NaN round-trips do
/// not spuriously fail.
fn tags_bitwise_eq(a: &Tag, b: &Tag) -> bool {
    match (a, b) {
        (Tag::Float(x), Tag::Float(y)) => x.to_bits() == y.to_bits(),
        (Tag::Double(x), Tag::Double(y)) => x.to_bits() == y.to_bits(),
        (Tag::List(x), Tag::List(y)) => {
            x.element_type == y.element_type
                && x.elements.len() == y.elements.len()
                && x.elements
                    .iter()
                    .zip(&y.elements)
                    .all(|(p, q)| tags_bitwise_eq(p, q))
        }
        (Tag::Compound(x), Tag::Compound(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((kx, vx), (ky, vy))| kx == ky && tags_bitwise_eq(vx, vy))
        }
        _ => a == b,
    }
}

#[test]
fn anonymous_network_values_accept_component_roots_without_relaxing_compound_api() {
    for tag in [
        Tag::String("Health".to_owned()),
        Tag::List(ListTag {
            element_type: tag_type::STRING,
            elements: vec![Tag::String("A".to_owned()), Tag::String("B".to_owned())],
        }),
        Tag::Compound(vec![("text".to_owned(), Tag::String("Health".to_owned()))]),
    ] {
        let mut encoded = Vec::new();
        write_network_value(&mut encoded, &tag).unwrap();
        let mut input = encoded.as_slice();
        assert_eq!(read_network_value(&mut input).unwrap(), tag);
        assert!(input.is_empty());
        if !matches!(tag, Tag::Compound(_)) {
            assert!(read_network(&mut encoded.as_slice()).is_err());
            assert!(write_network(&mut Vec::new(), &tag).is_err());
        }
    }
    assert!(read_network_value(&mut &[tag_type::END][..]).is_err());
    assert!(read_network_value(&mut &[tag_type::STRING, 0, 4, b'a'][..]).is_err());
    let invalid = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::String("wrong type".to_owned())],
    });
    let mut destination = vec![42];
    assert!(write_network_value(&mut destination, &invalid).is_err());
    assert_eq!(destination, [42], "preflight is atomic");
}
