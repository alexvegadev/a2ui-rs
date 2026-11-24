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

    for variant in ["caption", "body"] {
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
fn v1_null_deletes_a_key_or_clears_the_root_and_preserves_array_indices() {
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
    let error = document
        .apply_json(
            r#"{"version":"1.0","updateDataModel":{"surfaceId":"one","path":"/notes/temporary"}}"#,
        )
        .unwrap_err();
    assert_eq!(error.path(), Some("/updateDataModel/value"));
    assert_eq!(
        document.surface("one").unwrap().data_model()["notes"],
        json!({ "temporary": 9 })
    );
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "path": "/notes/temporary", "value": null }
        }),
    );
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "path": "/a~1b/0", "value": null }
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
    apply(
        &mut document,
        json!({
            "version": "1.0", "updateDataModel": { "surfaceId": "one", "value": null }
        }),
    );
    assert_eq!(document.surface("one").unwrap().data_model(), &json!({}));
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
                    "@checks": [{ "condition": { "@call": "and", "args": { "values": [
                        { "@call": "required", "args": { "value": { "@path": "name" } } },
                        { "@path": "/enabled" }
                    ] } }, "message": "Enter a name." }],
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
    surface
        .write_binding("/enabled", "", json!({"valid": true}))
        .unwrap();
    assert!(surface.action("confirm", "/rows/0").is_none());
    surface.write_binding("/enabled", "", json!(0)).unwrap();
    surface
        .write_binding("name", "/rows/0", json!("Avery"))
        .unwrap();
    assert!(surface.action("confirm", "/rows/0").is_none());
    surface
        .write_binding("/enabled", "", json!({"valid": true}))
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
    for content in [r#","value":null"#, ""] {
        document.apply_json(&format!(r#"{{"version":"v0.9","updateDataModel":{{"surfaceId":"legacy","path":"/total"{content}}}}}"#)).unwrap();
        assert_eq!(
            document
                .surface("legacy")
                .unwrap()
                .data_model()
                .get("total"),
            (!content.is_empty()).then_some(&Value::Null)
        );
    }
}

#[test]
fn v1_rejects_invalid_payloads_before_changing_the_document() {
    let mut document = Document::default();
    for (payload, path) in [
        (
            json!({"surfaceId": "s", "theme": {}}),
            "/createSurface/theme",
        ),
        (
            json!({"surfaceId": "s", "dataModel": "invalid"}),
            "/createSurface/dataModel",
        ),
        (
            json!({"surfaceId": "s", "components": []}),
            "/createSurface/components",
        ),
    ] {
        let message = json!({"version": "1.0", "createSurface": payload});
        let error = document.apply_json(&message.to_string()).unwrap_err();
        assert_eq!(error.path(), Some(path));
        assert!(document.surfaces().is_empty());
    }

    apply(
        &mut document,
        json!({
            "version": "1.0", "createSurface": {
                "surfaceId": "s", "catalogId": BASIC_CATALOG_V1,
                "metadata": {"extensions": {"host": true}},
                "components": [{"id": "root", "component": "Column", "children": ["label"]}]
            }
        }),
    );
    for (component, path) in [
        (
            json!({"id": "bad-id", "component": "Text", "text": "x"}),
            "/id",
        ),
        (json!({"id": "label", "component": "Surface"}), "/component"),
        (
            json!({"id": "label", "component": "Text", "text": "x", "variant": "h1"}),
            "/variant",
        ),
        (
            json!({"id": "label", "component": "Text", "text": "x", "checks": []}),
            "/checks",
        ),
        (
            json!({"id": "label", "component": "Text", "text": {"other": "x"}}),
            "/text",
        ),
        (
            json!({"id": "label", "component": "Button", "child": "any", "action": {"event": {"name": "submit"}}, "@checks": [{"message": "Missing condition"}]}),
            "/@checks",
        ),
        (
            json!({"id": "label", "component": "Button", "child": "any", "action": {"event": {"name": "submit"}}, "@checks": [{"condition": {"@call": "and", "args": {"values": []}}}]}),
            "/@checks",
        ),
        (
            json!({"id": "label", "component": "Card", "child": "root"}),
            "",
        ),
    ] {
        let message = json!({
            "version": "1.0", "updateComponents": {"surfaceId": "s", "components": [
                component, {"id": "sibling", "component": "Text", "text": "should not be added"}
            ]}
        });
        let error = document.apply_json(&message.to_string()).unwrap_err();
        let prefix = if path.is_empty() {
            "/updateComponents/components"
        } else {
            "/updateComponents/components/0"
        };
        assert_eq!(error.path(), Some(format!("{prefix}{path}").as_str()));
        let surface = document.surface("s").unwrap();
        assert_eq!(
            surface.component("root").unwrap().property("children"),
            &json!(["label"])
        );
        assert!(surface.component("label").is_none());
        assert!(surface.component("sibling").is_none());
    }
    let error = document
        .apply_json(r#"{"version":"2.0","deleteSurface":{"surfaceId":"s"}}"#)
        .unwrap_err();
    assert_eq!(error.path(), Some("/version"));
    assert!(document.surface("s").is_some());
}

#[test]
fn embedded_demo_has_a_complete_surface_and_a_valid_initial_action() {
    let mut document = Document::default();
    for line in include_str!("../examples/reservation.jsonl").lines() {
        document.apply_json(line).unwrap();
    }
    let surface = document.surface_mut("reservation").unwrap();
    assert_eq!(surface.component("root").unwrap().kind, "Column");
    assert_eq!(
        surface.action("confirm", "").unwrap().action.context["guests"],
        2
    );
    surface
        .write_binding("/booking/email", "", json!("invalid"))
        .unwrap();
    assert!(surface.action("confirm", "").is_none());
}
