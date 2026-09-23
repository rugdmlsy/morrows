use super::*;
use axum::extract::Query;
use morrows_core::{CreateConversation, SendLaunchInstruction};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list).post(create))
        .route("/conversations/{id}", get(get_one))
        .route(
            "/conversations/{id}/messages",
            get(messages).post(message_create),
        )
}

#[derive(Debug, Deserialize)]
struct ConversationListQuery {
    agent_instance_id: Option<Id>,
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ConversationListQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .list_conversations(query.agent_instance_id)
            .await?
    )))
}

async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateConversation>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.create_conversation(input).await?)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.get_conversation(id).await?)))
}

#[derive(Debug, Deserialize)]
struct ConversationMessagesQuery {
    before: Option<Id>,
    after: Option<Id>,
    #[serde(default = "default_message_limit")]
    limit: i64,
}

fn default_message_limit() -> i64 {
    80
}

async fn messages(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Query(query): Query<ConversationMessagesQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .conversation_history(id, query.before, query.after, query.limit)
            .await?
    )))
}

#[derive(Debug, Deserialize)]
struct CreateConversationMessageBody {
    body: String,
}

async fn message_create(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<CreateConversationMessageBody>,
) -> Result<Json<Value>, ApiError> {
    let message = state
        .store
        .create_human_conversation_message(id, &input.body)
        .await?;
    let conversation = state.store.get_conversation(id).await?;
    for attempt_id in state
        .store
        .active_launch_attempts_for_agent(conversation.agent_instance_id)
        .await?
    {
        let _ = state
            .store
            .send_launch_instruction(
                SendLaunchInstruction {
                    launch_attempt_id: attempt_id,
                    body: format!(
                        "New direct Morrows conversation message is queued in conversation {}. Check conversation_get for that conversation and reply with conversation_reply when appropriate.",
                        conversation.id
                    ),
                },
                None,
            )
            .await;
    }
    Ok(Json(json!(message)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn response_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn conversation_list_is_summary_only_and_history_is_paged() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("chat-agent", &[]).await.unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let create_request = Request::builder()
            .method("POST")
            .uri("/conversations")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"agent_instance_id":agent.id,"title":"Design chat"}).to_string(),
            ))
            .unwrap();
        let (status, conversation) = response_json(&app, create_request).await;
        assert_eq!(status, StatusCode::OK);
        let conversation_id = conversation["id"].as_str().unwrap();

        for body in ["one", "two", "three"] {
            let request = Request::builder()
                .method("POST")
                .uri(format!("/conversations/{conversation_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"body":body}).to_string()))
                .unwrap();
            assert_eq!(response_json(&app, request).await.0, StatusCode::OK);
        }

        let (status, summaries) = response_json(
            &app,
            Request::builder()
                .uri("/conversations")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summaries[0]["message_count"], 3);
        assert_eq!(summaries[0]["last_message_preview"], "three");
        assert!(summaries[0].get("messages").is_none());

        let (status, history) = response_json(
            &app,
            Request::builder()
                .uri(format!("/conversations/{conversation_id}/messages?limit=2"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);
        assert_eq!(history["messages"][0]["body"], "two");
        assert_eq!(history["messages"][1]["body"], "three");
        assert_eq!(history["has_more"], true);

        let before = history["next_before"].as_str().unwrap();
        let older = response_json(
            &app,
            Request::builder()
                .uri(format!(
                    "/conversations/{conversation_id}/messages?limit=2&before={before}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .1;
        assert_eq!(older["messages"][0]["body"], "one");
        assert_eq!(older["has_more"], false);
    }
}
