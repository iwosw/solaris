use super::*;
use mc_data::village_data::PoolAliasSpec;

#[test]
fn modern_jigsaw_alias_groups_are_correlated_and_seeded() {
    let direct = |alias: &str, target: &str| PoolAliasSpec::Direct {
        alias: static_id(alias),
        target: static_id(target),
    };
    let aliases = vec![
        PoolAliasSpec::RandomGroup {
            groups: vec![
                (
                    1,
                    vec![
                        direct("test:ranged", "test:skeleton"),
                        direct("test:slow", "test:slow_skeleton"),
                    ],
                ),
                (
                    1,
                    vec![
                        direct("test:ranged", "test:stray"),
                        direct("test:slow", "test:slow_stray"),
                    ],
                ),
            ],
        },
        PoolAliasSpec::Random {
            alias: static_id("test:melee"),
            targets: vec![(1, static_id("test:zombie")), (1, static_id("test:spider"))],
        },
    ];
    let mut ranged = std::collections::BTreeSet::new();
    let mut melee = std::collections::BTreeSet::new();
    for seed in 0..128 {
        let seed = seed * 341_873_128_712;
        let first = sample_aliases(&aliases, &mut LegacyRandom::new(seed));
        assert_eq!(
            first,
            sample_aliases(&aliases, &mut LegacyRandom::new(seed))
        );
        let chosen = first[&static_id("test:ranged")].path();
        assert_eq!(
            first[&static_id("test:slow")].path(),
            format!("slow_{chosen}")
        );
        ranged.insert(first[&static_id("test:ranged")].clone());
        melee.insert(first[&static_id("test:melee")].clone());
    }
    assert_eq!(ranged.len(), 2);
    assert_eq!(melee.len(), 2);
}
