use a2ui_rs::{BASIC_CATALOG_V1, BASIC_CATALOG_V09, Document};
use serde_json::{Value, json};

fn apply(document: &mut Document, message: Value) {
    document.apply_json(&message.to_string()).unwrap();
}

#[test]
fn incremental_messages_replace_components_and_allow_surface_ids_to_be_reused() {
    let mut document = Document::default();
    apply(
        &mut document,
        json!({
            "version": "1.0", "createSurface": { "surfaceId": "one", "catalogId": BASIC_CATALOG_V1,
                "components": [{ "id": "root", "component": "Column", "children": ["label"] }],
                "dataModel": { "name": "Grace" }
            }
        }),
    );
    assert!(
        document
            .surface("one")
            .unwrap()
            .component("label")
            .is_none()
    );

    for variant in ["h1", "h3"] {
        apply(
            &mut document,
            json!({
                "version": "1.0", "updateComponents": { "surfaceId": "one", "components": [
                    { "id": "label", "component": "Text", "variant": variant, "text": { "@path": "/name" } }
                ]}
            }),
        );
        let surface = document.surface("one").unwrap();
        let label = surface.component("label").unwrap();
        assert_eq!(label.property("variant"), variant);
        assert_eq!(surface.resolve(label.property("text"), ""), "Grace");
    }

    assert!(
        document
            .apply_json(
                r#"{"version":"1.0","deleteSurface":{"surfaceId":"one"},"createSurface":{}}"#
            )
            .is_err()
    );
    assert!(document.surface("one").is_some());
    apply(
        &mut document,
        json!({ "version": "1.0", "deleteSurface": { "surfaceId": "one" } }),
    );
    assert!(document.surface("one").is_none());
    apply(
        &mut document,
        json!({
            "version": "1.0", "createSurface": { "surfaceId": "one", "catalogId": BASIC_CATALOG_V1 }
        }),
    );
    let recreated = document.surface("one").unwrap();
    assert!(recreated.component("root").is_none());
    assert_eq!(recreated.data_model(), &json!({}));
}

#[test]
fn data_updates_distinguish_null_and_removal_and_preserve_array_indices() {
    let mut document = Document::default();
    apply(
        &mut document,
        json!({
            "version": "1.0", "createSurface": {
                "surfaceId": "one", "catalogId": BASIC_CATALOG_V1,
                "dataModel": { "a/b": [1, 2], "notes": { "temporary": 9 } }
            }
        }),
    );
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "path": "/notes/temporary", "value": null }
        }),
    );
    assert_eq!(
        document.surface("one").unwrap().data_model()["notes"],
        json!({ "temporary": null })
    );
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "path": "/notes/temporary" }
        }),
    );
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "path": "/a~1b/0" }
        }),
    );
    assert_eq!(
        document.surface("one").unwrap().data_model(),
        &json!({ "a/b": [null, 2], "notes": {} })
    );
    assert!(document.apply_json(r#"{"version":"1.0","updateDataModel":{"surfaceId":"one","path":"/a~1b/5","value":3}}"#).is_err());
    assert_eq!(
        document.surface("one").unwrap().data_model()["a/b"],
        json!([null, 2])
    );
}

#[test]
fn edits_resolve_template_scope_and_are_captured_in_actions() {
    let mut document = Document::default();
    apply(
        &mut document,
        json!({
            "version": "1.0", "createSurface": {
                "surfaceId": "one", "catalogId": BASIC_CATALOG_V1, "sendDataModel": true,
                "dataModel": { "host": "Kitchen", "rows": [{ "name": "" }, { "name": "Sophie" }] },
                "components": [{
                    "id": "confirm", "component": "Button", "child": "label",
                    "checks": [{ "condition": { "@call": "required", "args": { "value": { "@path": "name" } } }, "message": "Enter a name." }],
                    "action": { "event": {
                        "name": "confirm",
                        "userMessage": { "@call": "formatString", "args": { "value": "Book for ${name} at ${/host}" } },
                        "context": { "attendee": { "name": { "@path": "name" }, "host": { "@path": "/host" } } }
                    }}
                }]
            }
        }),
    );
    let surface = document.surface_mut("one").unwrap();
    assert!(surface.action("confirm", "/rows/0").is_none());
    surface
        .write_binding("name", "/rows/0", json!("Avery"))
        .unwrap();

    let action = surface.action("confirm", "/rows/0").unwrap();
    let wire = serde_json::to_value(&action).unwrap();
    assert_eq!(wire["version"], "1.0");
    assert_eq!(wire["action"]["sourceComponentId"], "confirm");
    assert_eq!(
        wire["action"]["context"],
        json!({ "attendee": { "name": "Avery", "host": "Kitchen" } })
    );
    assert_eq!(wire["action"]["userMessage"], "Book for Avery at Kitchen");
    assert!(wire.get("dataModel").is_none());
    assert_eq!(
        action.metadata().unwrap()["a2uiRendererDataModel"]["surfaces"]["one"]["rows"][1]["name"],
        "Sophie"
    );
}

#[test]
fn legacy_wire_bindings_and_registered_formatting_work() {
    let mut document = Document::default();
    apply(
        &mut document,
        json!({
            "version": "v0.9.1", "createSurface": { "surfaceId": "legacy", "catalogId": BASIC_CATALOG_V09 }
        }),
    );
    apply(
        &mut document,
        json!({
            "version": "v0.9", "updateDataModel": { "surfaceId": "legacy", "value": { "total": 1234.5 } }
        }),
    );
    let surface = document.surface("legacy").unwrap();
    let formatted = surface.resolve(&json!({
        "call": "formatString", "args": { "value": "Total: ${formatNumber(value:${/total}, decimals:2)}" }
    }), "");
    assert_eq!(formatted, "Total: 1,234.50");
}

#[test]
fn embedded_demo_has_a_complete_surface_and_a_valid_initial_action() {
    let mut document = Document::default();
    for line in include_str!("../examples/reservation.jsonl").lines() {
        document.apply_json(line).unwrap();
    }
    let surface = document.surface("reservation").unwrap();
    assert_eq!(surface.component("root").unwrap().kind, "Column");
    assert_eq!(
        surface.action("confirm", "").unwrap().action.context["guests"],
        2
    );
}
