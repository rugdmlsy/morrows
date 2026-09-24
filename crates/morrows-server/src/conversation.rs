use super::*;
use axum::extract::Query;
use morrows_core::{CreateConversation, CreateConversationSummaryRevision};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list).post(create))
        .route("/conversations/{id}", get(get_one))
        .route(
            "/conversations/{id}/messages",
            get(messages).post(message_create),
        )
        .route(
            "/conversations/{id}/summary",
            get(summary_get).post(summary_create),
        )
        .route("/conversations/{id}/summary/history", get(summary_history))
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
    Ok(Json(json!(
        state
            .store
            .create_human_conversation_message(id, &input.body)
            .await?
    )))
}

async fn summary_get(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .get_latest_conversation_summary_revision(id)
            .await?
    )))
}

async fn summary_create(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(mut input): Json<CreateConversationSummaryRevision>,
) -> Result<Json<Value>, ApiError> {
    input.conversation_id = id;
    Ok(Json(json!(
        state
            .store
            .create_conversation_summary_revision(input)
            .await?
    )))
}

async fn summary_history(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .list_conversation_summary_revisions(id)
            .await?
    )))
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

    #[tokio::test]
    async fn conversation_summary_rest_routes_support_lifecycle_and_history() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("summary-agent", &[]).await.unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let create_request = Request::builder()
            .method("POST")
            .uri("/conversations")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"agent_instance_id": agent.id, "title": "Summary Test"}).to_string(),
            ))
            .unwrap();
        let (status, conversation) = response_json(&app, create_request).await;
        assert_eq!(status, StatusCode::OK);
        let conversation_id = conversation["id"].as_str().unwrap();

        // 1. Initial GET summary returns null
        let (status, initial_summary) = response_json(
            &app,
            Request::builder()
                .uri(format!("/conversations/{conversation_id}/summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(initial_summary.is_null());

        // 2. POST create first summary revision
        let post_summary1 = Request::builder()
            .method("POST")
            .uri(format!("/conversations/{conversation_id}/summary"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "goal": "Initial assessment",
                    "current_state": "Analyzing logs",
                    "deterministic_facts": {"files_inspected": 2},
                    "created_by": "system"
                })
                .to_string(),
            ))
            .unwrap();
        let (status, summary1) = response_json(&app, post_summary1).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summary1["goal"], "Initial assessment");
        assert_eq!(summary1["current_state"], "Analyzing logs");
        let rev1_id = summary1["id"].as_str().unwrap();

        // 3. GET summary returns the latest (revision 1)
        let (status, latest1) = response_json(
            &app,
            Request::builder()
                .uri(format!("/conversations/{conversation_id}/summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(latest1["id"], rev1_id);

        // 4. POST create second summary revision referencing previous
        let post_summary2 = Request::builder()
            .method("POST")
            .uri(format!("/conversations/{conversation_id}/summary"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "previous_revision_id": rev1_id,
                    "goal": "Fix crash",
                    "current_state": "Patch applied",
                    "created_by": "agent"
                })
                .to_string(),
            ))
            .unwrap();
        let (status, summary2) = response_json(&app, post_summary2).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summary2["goal"], "Fix crash");
        assert_eq!(summary2["previous_revision_id"], rev1_id);

        // 5. GET summary/history returns both in reverse chronological order
        let (status, history) = response_json(
            &app,
            Request::builder()
                .uri(format!("/conversations/{conversation_id}/summary/history"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let list = history.as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], summary2["id"]);
        assert_eq!(list[1]["id"], rev1_id);
    }
}
