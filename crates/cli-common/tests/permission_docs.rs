//! The permission vocabulary lives in `Capability::HOST_FACING`; the pages that
//! list it are copies a reader trusts. This holds each copy to the code, so a
//! capability added with its flag but not its documentation fails here rather
//! than reading, on the security pages, as a grant that does not exist.

use std::path::{Path, PathBuf};

use es_runtime_cli_common::permissions::scope_hint;
use es_runtime_common::Capability;

/// The repository root, two levels above this crate.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every page that lists the permission names for a reader.
const PAGES: [&str; 6] = [
    "README.md",
    "SECURITY.md",
    "docs/API.md",
    "docs/ARCHITECTURE.md",
    "website/app/docs/security/permissions/page.mdx",
    "website/app/api/cli/page.mdx",
];

fn names() -> Vec<&'static str> {
    Capability::HOST_FACING
        .into_iter()
        .filter_map(Capability::flag_name)
        .collect()
}

fn read(page: &str) -> String {
    std::fs::read_to_string(repo().join(page)).unwrap_or_else(|e| panic!("{page}: {e}"))
}

#[test]
fn every_page_lists_every_permission_name() {
    let mut missing = Vec::new();
    for page in PAGES {
        let text = read(page);
        for name in names() {
            // ARCHITECTURE writes the list as one code span, `read, write, …`.
            let listed = text.contains(&format!("`{name}`"))
                || text.contains(&format!("--allow-{name}`"))
                || text.contains(&format!("`{name},"))
                || text.contains(&format!(", {name},"))
                || text.contains(&format!(", {name}`"));
            if !listed {
                missing.push(format!("{page}: `{name}`"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "permission names missing from docs:\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_scopable_names_are_the_ones_the_pages_say() {
    // The sentence each page uses for the scopable set, built from the code.
    let scopable: Vec<_> = Capability::HOST_FACING
        .into_iter()
        .filter(|cap| scope_hint(*cap).is_some())
        .filter_map(Capability::flag_name)
        .map(|name| format!("`{name}`"))
        .collect();
    let (last, rest) = scopable.split_last().expect("some capability takes a list");
    let sentence = format!("{} and {last}", rest.join(", "));
    for page in [
        "README.md",
        "SECURITY.md",
        "docs/API.md",
        "website/app/docs/security/permissions/page.mdx",
    ] {
        let text = read(page).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            text.contains(&sentence),
            "{page} does not name the scopable permissions as {sentence}"
        );
    }
}

#[test]
fn no_page_counts_the_names_wrong() {
    // A count is the first thing to go stale; any page that keeps one must
    // keep the right one.
    let total = names().len();
    let words = [
        "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen",
    ];
    let right = match total {
        8..=14 => words[total - 8],
        _ => panic!("extend the count words to {total}"),
    };
    for page in PAGES {
        let text = read(page).to_lowercase();
        for word in words.iter().filter(|w| **w != right) {
            for phrase in [format!("of the {word}"), format!("the {word} names")] {
                assert!(
                    !text.contains(&phrase),
                    "{page} says \"{phrase}\"; there are {right}"
                );
            }
        }
    }
}
