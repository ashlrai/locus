//! Keep the actor guide discoverable and its source references resolvable.

const GUIDE: &str = include_str!("../../../docs/registry-trust-actors.md");
const SOURCE: &str = include_str!("../../../docs/registry-trust.md");

#[test]
fn guide_covers_each_source_actor_and_is_linked_from_source() {
    let table = SOURCE
        .split("## Trust model\n")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();
    for actor in ["Operator", "Consumer", "CI"] {
        assert!(table.contains(&format!("**{actor}**")));
        assert!(GUIDE.contains(&format!("| **{actor}** |")));
    }
    assert!(SOURCE.contains("](./registry-trust-actors.md)"));
}

#[test]
fn source_section_links_resolve() {
    let prefix = "](./registry-trust.md#";
    let mut count = 0;
    for suffix in GUIDE.split(prefix).skip(1) {
        let anchor = suffix.split(')').next().unwrap();
        let found = SOURCE.lines().filter(|line| line.starts_with('#')).any(|line| {
            let heading = line.trim_start_matches('#').trim().to_ascii_lowercase();
            let slug: String = heading
                .chars()
                .filter_map(|ch| match ch {
                    ' ' => Some('-'),
                    '-' | 'a'..='z' | '0'..='9' => Some(ch),
                    _ => None,
                })
                .collect();
            slug == anchor
        });
        assert!(found, "missing source section: {anchor}");
        count += 1;
    }
    assert!(count >= 6, "actor responsibilities need source sections");
}
