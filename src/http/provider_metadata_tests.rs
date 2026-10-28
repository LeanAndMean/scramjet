use super::*;
use serde_json::json;
#[test]
fn startup_refuses_changed_or_unavailable_route_without_fallback() {
    let status = json!({"startup_ready":true,"transition_state":"ready","version":"0.15.0-beta.1","model_catalog":{"source":"agentic","stale":false}});
    let models = json!({"data":[{"id":"gpt-6-luna"}]});
    let cards = json!({"revision":"2026-10-26-001","models":[{"requestId":"gpt-6-luna","provider":"openai","primaryProtocol":"openai-responses","supportedProtocols":["openai-responses"],"limits":{"context":1050000,"output":128000}}]});
    assert!(compatible(&status, &models, &cards));
    for key in [
        "startup_ready",
        "transition_state",
        "version",
        "model_catalog",
    ] {
        let mut status = status.clone();
        status[key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    for key in ["source", "stale"] {
        let mut status = status.clone();
        status["model_catalog"][key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    for key in [
        "requestId",
        "provider",
        "primaryProtocol",
        "supportedProtocols",
        "limits",
    ] {
        let mut cards = cards.clone();
        cards["models"][0][key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    let mut models = models.clone();
    models["data"] = json!([]);
    assert!(!compatible(&status, &models, &cards));
    let mut cards = cards.clone();
    cards["revision"] = json!("changed");
    assert!(!compatible(&status, &models, &cards));
}
