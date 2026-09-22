#[path = "plan_snapshots/fields.rs"]
mod fields;

use fields::Fields;
use serde::Deserialize;
use serde_json::{Value, json};

fn select(config: &str, mut value: Value) -> Value {
    #[derive(Deserialize)]
    struct Config {
        fields: Fields,
    }

    let config: Config = toml::from_str(config).unwrap();
    config.fields.select(&mut value).unwrap();
    value
}

#[test]
fn finds_fields_in_nested_plans_without_leaking_unrelated_fields() {
    let input = json!({
        "graph": [{
            "key": ["workspace", "outer"],
            "items": [{"Expanded": {"graph": [{
                "key": ["workspace", "inner"],
                "Spawn": {"cache_metadata": {
                    "execution_cache_key": {"task": "inner", "extra_args": []},
                    "remote_cache": {"mode": "off"}
                }},
                "neighbors": []
            }], "concurrency_limit": 4}}],
            "neighbors": []
        }],
        "concurrency_limit": 4
    });
    assert_eq!(
        select(
            "fields = { key = true, neighbors = true, cache_metadata = { execution_cache_key = true } }",
            input
        ),
        json!({"graph": [{
            "key": ["workspace", "outer"],
            "items": [{"Expanded": {"graph": [{
                "key": ["workspace", "inner"],
                "Spawn": {"cache_metadata": {
                    "execution_cache_key": {"task": "inner", "extra_args": []}
                }},
                "neighbors": []
            }]}}],
            "neighbors": []
        }]})
    );
}

#[test]
fn nested_selection_is_scoped_to_direct_children() {
    assert_eq!(
        select(
            "fields = { parent = { keep = true } }",
            json!({
                "keep": "outside",
                "wrapper": {"parent": {
                    "keep": {"all": [1, null, {}]},
                    "nested": {"keep": "too deep"}
                }}
            })
        ),
        json!({"wrapper": {"parent": {"keep": {"all": [1, null, {}]}}}})
    );
}

#[test]
fn preserves_array_positions_and_distinguishes_selected_null() {
    assert_eq!(
        select(
            "fields = { keep = true }",
            json!({"items": [
                {"discard": 1},
                {"keep": null},
                {"discard": 2},
                [{"keep": []}, {"discard": 3}]
            ]})
        ),
        json!({"items": [
            "<unselected>", {"keep": null}, "<unselected>",
            [{"keep": []}, "<unselected>"]
        ]})
    );
}

#[test]
fn nested_selection_preserves_null_empty_collections_and_scalar_changes() {
    assert_eq!(
        select(
            "fields = { parent = { keep = true } }",
            json!({"items": [
                {"parent": null}, {"parent": {}}, {"parent": []},
                {"parent": false}, {"parent": 5}, {"parent": "changed"},
                {"parent": [{"keep": 1, "discard": 2}, {"keep": 3}]}
            ]})
        ),
        json!({"items": [
            {"parent": null}, {"parent": {}}, {"parent": []},
            {"parent": false}, {"parent": 5}, {"parent": "changed"},
            {"parent": [{"keep": 1}, {"keep": 3}]}
        ]})
    );
}

#[test]
fn true_keeps_the_full_snapshot() {
    let input = json!({"graph": [{"key": "build", "items": []}], "limit": 4});
    assert_eq!(select("fields = true", input.clone()), input);
}

#[test]
fn rejects_invalid_selections_during_deserialization() {
    for config in [json!(false), json!({"parent": false}), json!({"parent": ["keep"]})] {
        assert!(serde_json::from_value::<Fields>(config).is_err());
    }
}

#[test]
fn rejects_empty_selections_when_visited() {
    for (config, mut snapshot) in [
        (json!({}), json!({})),
        (json!({"parent": {}}), json!({"parent": null})),
        (json!({"parent": {"nested": {}}}), json!({"parent": {"nested": null}})),
        (json!({"parent": {"nested": {}}}), json!({"parent": [{}, {"nested": null}]})),
        (json!({"parent": {}}), json!({"items": [{}, {"parent": null}]})),
    ] {
        let fields: Fields = serde_json::from_value(config).unwrap();
        assert_eq!(fields.select(&mut snapshot).unwrap_err(), "fields must be a nonempty table");
    }
}

#[test]
fn does_not_validate_empty_selections_for_absent_fields() {
    assert_eq!(
        select("fields = { key = true, missing = {} }", json!({"key": "build", "noise": 1})),
        json!({"key": "build"})
    );
    assert_eq!(
        select(
            "fields = { parent = { keep = true, missing = {} } }",
            json!({"parent": {"keep": 1, "noise": 2}})
        ),
        json!({"parent": {"keep": 1}})
    );
    assert_eq!(
        select("fields = { parent = { missing = {} } }", json!({"parent": null})),
        json!({"parent": null})
    );
}

#[test]
fn rejects_selections_that_match_no_fields() {
    let fields: Fields = serde_json::from_value(json!({"missing": true})).unwrap();
    assert!(fields.select(&mut json!({"graph": [{"key": "build"}]})).is_err());
}
