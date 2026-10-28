//! Offline wire contract for Phantom consumers. Never mint credentials.
use serde_json::{json, Value};

fn schema(fragment: &str) -> jsonschema::Validator {
    let mut schema: Value =
        serde_json::from_str(include_str!("../../../schema/mcp-grant.v1.schema.json")).unwrap();
    if !fragment.is_empty() {
        // Keep local definitions in scope without fetching the published URI.
        schema["$ref"] = json!(format!("#/$defs/{fragment}"));
        let object = schema.as_object_mut().unwrap();
        for key in ["type", "required", "properties", "additionalProperties"] {
            object.remove(key);
        }
    }
    jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

fn fixtures() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/mcp-grant.v1.json")).unwrap()
}

fn mint_fixture(row: &Value) -> Value {
    let mut mint = row.clone();
    for key in ["expired", "revoked", "live_http_sessions"] {
        mint.as_object_mut().unwrap().remove(key);
    }
    // Shape-only synthetic bytes, never issued by Locus or usable as authority.
    mint["token"] = json!(format!(
        "lmt_{}.{}",
        row["grant_id"].as_str().unwrap(),
        "0".repeat(64)
    ));
    mint["serve"] = json!("locus-mcp --http --multi-tenant");
    mint
}

#[test]
fn fixture_grants_validate_for_mint_and_list() {
    let rows = fixtures();
    assert!(schema("list").is_valid(&json!(rows)));
    for row in rows {
        assert!(schema("listEntry").is_valid(&row));
        let mut mint = mint_fixture(&row);
        assert!(schema("").is_valid(&mint));
        for optional in ["label", "serve"] {
            mint.as_object_mut().unwrap().remove(optional);
        }
        assert!(schema("").is_valid(&mint));
    }
}

#[test]
fn unknown_fields_are_rejected_on_every_grant_surface() {
    for row in fixtures() {
        for (fragment, mut grant) in [("", mint_fixture(&row)), ("listEntry", row)] {
            for field in ["unknown", "scope", "providers", "schema_version"] {
                grant[field] = json!(true);
                assert!(!schema(fragment).is_valid(&grant), "accepted {field}");
                grant.as_object_mut().unwrap().remove(field);
            }
        }
    }
    let mut rows = fixtures();
    rows[0]["token"] = json!("forbidden");
    assert!(!schema("list").is_valid(&json!(rows)));
}

#[test]
fn required_fields_and_types_fail_closed() {
    for (fragment, grant) in [
        ("", mint_fixture(&fixtures()[0])),
        ("listEntry", fixtures().remove(0)),
    ] {
        let validator = schema(fragment);
        for key in grant.as_object().unwrap().keys() {
            if ["label", "serve"].contains(&key.as_str()) {
                continue;
            }
            let mut missing = grant.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(!validator.is_valid(&missing), "accepted missing {key}");
            let mut wrong_type = grant.clone();
            wrong_type[key] = json!({});
            assert!(!validator.is_valid(&wrong_type), "accepted object {key}");
        }
        for (key, value) in [
            ("grant_id", json!("ABCDEF0123456789")),
            ("grant_id", json!("0123")),
            ("session_id", json!("ses_bad")),
            ("binding", json!("")),
            ("tenant", json!("")),
            ("expires_at", json!("tomorrow")),
            ("expires_at", json!("2030-02-30T00:00:00Z")),
            ("label", json!(false)),
        ] {
            let mut invalid = grant.clone();
            invalid[key] = value;
            assert!(!validator.is_valid(&invalid), "accepted invalid {key}");
        }
    }
    let mut mint = mint_fixture(&fixtures()[0]);
    mint["token"] = json!("malformed");
    assert!(!schema("").is_valid(&mint));
    let mut row = fixtures().remove(0);
    for invalid in [json!(-1), json!(1.5)] {
        row["live_http_sessions"] = invalid;
        assert!(!schema("listEntry").is_valid(&row));
    }
    assert!(!schema("list").is_valid(&json!({})));
    assert!(schema("list").is_valid(&json!([])));
}
