//! The `sm parse --json` schema.
//!
//! Future tooling and the M5 evaluation will read this format, so its shape is
//! pinned here rather than left to whatever `derive(Serialize)` happens to
//! produce.

mod support;

use serde_json::Value;
use sm_cst::JsonTree;
use support::parse_fixture;

fn json_for(name: &str) -> Value {
    let tree = parse_fixture(name);
    serde_json::to_value(JsonTree::new(&tree)).expect("the DTO must serialize")
}

#[test]
fn top_level_shape_is_stable() {
    let json = json_for("typical");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["language"], "java");
    assert_eq!(json["has_errors"], false);
    assert_eq!(json["root"], 0);
    assert!(json["source_len"].as_u64().unwrap() > 0);
    assert_eq!(
        json["node_count"].as_u64().unwrap(),
        json["nodes"].as_array().unwrap().len() as u64
    );
}

#[test]
fn node_ids_match_array_positions() {
    for name in support::FIXTURES {
        let json = json_for(name);
        for (i, node) in json["nodes"].as_array().unwrap().iter().enumerate() {
            assert_eq!(node["id"].as_u64().unwrap(), i as u64, "{name}");
        }
    }
}

#[test]
fn leaves_carry_text_and_interior_nodes_do_not() {
    let json = json_for("typical");
    for node in json["nodes"].as_array().unwrap() {
        let is_leaf = node["children"].as_array().unwrap().is_empty();
        assert_eq!(
            node.get("text").is_some(),
            is_leaf,
            "text should be present exactly on leaves: {node}"
        );
    }
}

#[test]
fn attachment_serializes_as_a_tagged_object() {
    let json = json_for("typical");
    // The fixture's copyright header is separated from `package` by a blank
    // line, so the trivia pass leaves it floating — the unit variant.
    let header = &json["nodes"][1];
    assert_eq!(header["kind"], "line_comment");
    assert_eq!(header["extra"], true);
    assert_eq!(header["attachment"]["kind"], "floating");
    assert!(header["attachment"].get("owner").is_none());
    assert!(header["leading_trivia"].as_array().unwrap().is_empty());

    // An attached comment carries its owner, and the owner lists it back.
    let tree = parse_fixture("typical");
    let lang = support::java();
    let (comment_id, owner_id) = tree
        .ids()
        .find_map(|id| match tree.node(id).attachment {
            sm_cst::Attachment::Leading(owner) => Some((id, owner)),
            _ => None,
        })
        .expect("the typical fixture has Javadoc that attaches");
    assert!(lang.is_comment(tree.node(comment_id).kind));

    let node = &json["nodes"][comment_id.index()];
    assert_eq!(node["attachment"]["kind"], "leading");
    assert_eq!(node["attachment"]["owner"], owner_id.0);
    assert!(
        json["nodes"][owner_id.index()]["leading_trivia"]
            .as_array()
            .unwrap()
            .contains(&Value::from(comment_id.0))
    );

    // And the trailing side, which uses the other tag.
    let (comment_id, owner_id) = tree
        .ids()
        .find_map(|id| match tree.node(id).attachment {
            sm_cst::Attachment::Trailing(owner) => Some((id, owner)),
            _ => None,
        })
        .expect("the typical fixture has a trailing comment");
    let node = &json["nodes"][comment_id.index()];
    assert_eq!(node["attachment"]["kind"], "trailing");
    assert_eq!(node["attachment"]["owner"], owner_id.0);
    assert!(
        json["nodes"][owner_id.index()]["trailing_trivia"]
            .as_array()
            .unwrap()
            .contains(&Value::from(comment_id.0))
    );
}

#[test]
fn broken_file_reports_errors_in_json() {
    let json = json_for("broken");
    assert_eq!(json["has_errors"], true);
    assert!(
        json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["error"] == true || n["missing"] == true)
    );
}

#[test]
fn every_fixture_serializes() {
    for name in support::FIXTURES {
        let json = json_for(name);
        assert!(!json["nodes"].as_array().unwrap().is_empty(), "{name}");
    }
}
