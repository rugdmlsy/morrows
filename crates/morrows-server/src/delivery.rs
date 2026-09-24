use super::*;
use axum::{extract::Query, http::HeaderMap};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent-deliveries", get(inbox))
        .route("/agent-deliveries/{id}/ack", post(ack))
}

#[derive(Debug, Deserialize)]
struct DeliveryInboxQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    80
}

async fn inbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeliveryInboxQuery>,
) -> Result<Json<Value>, ApiError> {
    let agent_id = collaboration::actor(&headers)?;
    Ok(Json(json!(
        state
            .store
            .agent_delivery_inbox(agent_id, query.limit)
            .await?
    )))
}

async fn ack(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let agent_id = collaboration::actor(&headers)?;
    Ok(Json(json!(
        state
            .store
            .acknowledge_agent_delivery(id, agent_id, &format!("external_bridge:{agent_id}"),)
            .await?
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use morrows_core::CreateConversation;
    use tower::ServiceExt;

    async fn request(
        app: &Router,
        method: &str,
        path: &str,
        actor: Option<Id>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(id) = actor {
            builder = builder.header("x-agent-instance-id", id.to_string());
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn delivery_bridge_scopes_inbox_and_ack_to_target_agent() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("bridge-a", &[]).await.unwrap();
        let b = store.register_agent("bridge-b", &[]).await.unwrap();
        let conversation = store
            .create_conversation(CreateConversation {
                agent_instance_id: a.id,
                title: "Bridge".into(),
            })
            .await
            .unwrap();
        store
            .create_human_conversation_message(conversation.id, "bridge message")
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        assert_eq!(
            request(&app, "GET", "/agent-deliveries", None).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&app, "GET", "/agent-deliveries", Some(b.id))
                .await
                .1
                .as_array()
                .unwrap()
                .len(),
            0
        );

        let (_, inbox) = request(&app, "GET", "/agent-deliveries", Some(a.id)).await;
        let delivery_id = inbox[0]["id"].as_str().unwrap();
        assert_eq!(inbox[0]["kind"], "conversation_message");

        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/agent-deliveries/{delivery_id}/ack"),
                Some(b.id),
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let (status, delivered) = request(
            &app,
            "POST",
            &format!("/agent-deliveries/{delivery_id}/ack"),
            Some(a.id),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(delivered["status"], "delivered");
    }
}
