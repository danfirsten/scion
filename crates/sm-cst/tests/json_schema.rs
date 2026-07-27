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
    let comment = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "line_comment")
        .expect("the typical fixture has a line comment");
    assert_eq!(comment["attachment"]["kind"], "floating");
    assert_eq!(comment["extra"], true);
    assert!(comment["leading_trivia"].as_array().unwrap().is_empty());

    // With an attachment recorded, the owner shows up in the payload.
    let mut tree = parse_fixture("typical");
    let lang = support::java();
    let comment_id = tree
        .ids()
        .find(|&id| lang.is_comment(tree.node(id).kind))
        .unwrap();
    let owner_id = tree
        .ids()
        .find(|&id| tree.node(id).kind == "class_declaration")
        .unwrap();
    tree.attach_leading(owner_id, comment_id);

    let json = serde_json::to_value(JsonTree::new(&tree)).unwrap();
    let node = &json["nodes"][comment_id.index()];
    assert_eq!(node["attachment"]["kind"], "leading");
    assert_eq!(node["attachment"]["owner"], owner_id.0);
    assert_eq!(
        json["nodes"][owner_id.index()]["leading_trivia"][0],
        comment_id.0
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
