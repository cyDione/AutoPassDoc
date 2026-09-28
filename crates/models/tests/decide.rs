use std::time::Duration;

use models::{
    Answer, AnswerValue, Client, ClientConfig, Error, Provider, ProviderKind, Question,
    resolve_profile,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> Client {
    Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        ..Default::default()
    })
    .unwrap()
}

fn questions() -> Vec<Question> {
    vec![
        Question::yes_no("addressed", "修改是否回应了批注？"),
        Question::choice(
            "action",
            "应如何处理这条批注？",
            [
                ("revise", "修改原文"),
                ("explain", "补充说明"),
                ("ignore", "无需处理"),
            ],
        ),
        Question::score(
            "quality",
            "修改质量如何？",
            ["很差", "较差", "一般", "较好", "很好"],
        ),
    ]
}

async fn server_answering(route: &str, response: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;
    server
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[tokio::test]
async fn jev_decide_sends_raw_request_and_parses_all_types() {
    let server = server_answering(
        "/v1/systemone",
        json!({
            "id": "dec_123",
            "answers": {
                "addressed": { "type": "noul", "noul": 0.87 },
                "action": {
                    "type": "choice",
                    "choice": "revise",
                    "probabilities": { "revise": 0.6, "explain": 0.3, "ignore": 0.0 },
                    "confidence": 0.9,
                    "extra": "ignored"
                },
                "quality": { "type": "score", "score": 0.73, "probabilities": [0.0, 0.1, 0.2, 0.4, 0.3] }
            },
            "usage": { "tokens": 321 }
        }),
    )
    .await;
    let p = Provider::new(ProviderKind::OpenAiCompatible, server.uri()).with_api_key("k");
    let answers = client()
        .decide(
            &p,
            "jev-latest",
            "批注：请补充数据来源。修改：已补充。",
            &questions(),
        )
        .await
        .unwrap();

    let req = &server.received_requests().await.unwrap()[0];
    let body: Value = req.body_json().unwrap();
    assert_eq!(
        body,
        json!({
            "model": "jev-latest",
            "state": "批注：请补充数据来源。修改：已补充。",
            "questions": {
                "addressed": { "type": "noul", "instructions": "修改是否回应了批注？" },
                "action": {
                    "type": "choice",
                    "instructions": "应如何处理这条批注？",
                    "criteria": { "revise": "修改原文", "explain": "补充说明", "ignore": "无需处理" }
                },
                "quality": {
                    "type": "score",
                    "instructions": "修改质量如何？",
                    "criteria": ["很差", "较差", "一般", "较好", "很好"]
                }
            }
        })
    );
    let raw = String::from_utf8(req.body.clone()).unwrap();
    let pos = |s: &str| raw.find(s).unwrap();
    assert!(pos("\"addressed\"") < pos("\"action\"") && pos("\"action\"") < pos("\"quality\""));
    assert!(pos("\"revise\"") < pos("\"explain\"") && pos("\"explain\"") < pos("\"ignore\""));

    assert_eq!(answers.len(), 3);
    assert_eq!(
        answers[0],
        Answer {
            key: "addressed".into(),
            value: AnswerValue::YesNo { p_yes: 0.87 }
        }
    );
    let AnswerValue::Choice {
        choice,
        probabilities,
        confidence,
    } = &answers[1].value
    else {
        panic!("{:?}", answers[1]);
    };
    assert_eq!(choice, "revise");
    assert_eq!(*confidence, 0.9);
    let labels: Vec<&str> = probabilities.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["revise", "explain", "ignore"]);
    assert!(approx(probabilities[0].1, 2.0 / 3.0) && approx(probabilities[1].1, 1.0 / 3.0));
    let AnswerValue::Score {
        normalized,
        expected_level,
        probabilities,
    } = &answers[2].value
    else {
        panic!("{:?}", answers[2]);
    };
    assert!(approx(*expected_level, 2.9), "{expected_level}");
    assert!(approx(*normalized, 2.9 / 4.0), "{normalized}");
    assert_eq!(probabilities.len(), 5);
}

#[tokio::test]
async fn jev_decide_parses_alternative_shapes() {
    let server = server_answering(
        "/api/v1/jev/decide",
        json!({
            "answers": {
                "addressed": { "type": "noul", "noul": "0.25" },
                "action": { "type": "choice", "probabilities": [0.1, 0.7, 0.2] },
                "quality": { "type": "score", "score": 3 }
            }
        }),
    )
    .await;
    let p = Provider::new(
        ProviderKind::OpenAiCompatible,
        format!("{}/api", server.uri()),
    )
    .with_decision_path("jev/decide");
    let answers = client()
        .decide(&p, "jev-latest", "s", &questions())
        .await
        .unwrap();
    assert_eq!(answers[0].value, AnswerValue::YesNo { p_yes: 0.25 });
    let AnswerValue::Choice {
        choice, confidence, ..
    } = &answers[1].value
    else {
        panic!();
    };
    assert_eq!(choice, "explain");
    assert!(approx(*confidence, 0.7));
    assert_eq!(
        answers[2].value,
        AnswerValue::Score {
            normalized: 0.75,
            expected_level: 3.0,
            probabilities: vec![]
        }
    );
}

#[tokio::test]
async fn normalized_score_maps_to_expected_level() {
    let server = server_answering(
        "/v1/systemone",
        json!({ "answers": { "quality": { "type": "score", "score": 0.5 } } }),
    )
    .await;
    let p = Provider::new(ProviderKind::OpenAiCompatible, server.uri());
    let answers = client()
        .decide(&p, "jev-latest", "s", &questions()[2..])
        .await
        .unwrap();
    let AnswerValue::Score {
        normalized,
        expected_level,
        ..
    } = answers[0].value
    else {
        panic!();
    };
    assert_eq!((normalized, expected_level), (0.5, 2.0));
}

#[tokio::test]
async fn missing_answer_names_the_key() {
    let server = server_answering(
        "/v1/systemone",
        json!({ "answers": { "addressed": { "type": "noul", "noul": 0.5 } } }),
    )
    .await;
    let p = Provider::new(ProviderKind::OpenAiCompatible, server.uri());
    let err = client()
        .decide(&p, "jev-latest", "s", &questions())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Decode(_)));
    assert!(err.to_string().contains("action"), "{err}");
}

#[tokio::test]
async fn rejects_invalid_questions_without_calling() {
    let p = Provider::new(ProviderKind::OpenAiCompatible, "http://127.0.0.1:1");
    let dup = vec![Question::yes_no("a", "x"), Question::yes_no("a", "y")];
    let one_level = vec![Question::score("s", "x", ["only"])];
    for qs in [dup, one_level] {
        let err = client().decide(&p, "jev", "s", &qs).await.unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)), "{err:?}");
    }
    assert!(
        client()
            .decide(&p, "jev", "s", &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn decide_via_chat_parses_fenced_json_and_normalizes() {
    let content = r#"好的，结果如下：
```json
{
  "answers": {
    "addressed": { "type": "noul", "noul": 1.3 },
    "action": { "type": "choice", "choice": "Explain", "probabilities": { "revise": 0.2, "explain": 0.6, "ignore": 0.4 } },
    "quality": { "type": "score", "score": 0.6, "probabilities": [0, 0, 1, 2, 1] }
  }
}
```"#;
    let server = server_answering(
        "/v1/chat/completions",
        json!({ "choices": [{ "message": { "content": content }, "finish_reason": "stop" }] }),
    )
    .await;
    let p = Provider::new(ProviderKind::OpenAiCompatible, server.uri()).with_api_key("k");
    let model = "cline-pass/deepseek-v4.1-flash";
    let answers = client()
        .decide_via_chat(
            &p,
            model,
            &resolve_profile(model, None, None),
            "材料",
            &questions(),
        )
        .await
        .unwrap();

    assert_eq!(answers[0].value, AnswerValue::YesNo { p_yes: 1.0 });
    let AnswerValue::Choice {
        choice,
        probabilities,
        confidence,
    } = &answers[1].value
    else {
        panic!();
    };
    assert_eq!(choice, "explain");
    assert!(approx(probabilities[1].1, 0.5) && approx(*confidence, 0.5));
    let sum: f32 = probabilities.iter().map(|(_, p)| p).sum();
    assert!(approx(sum, 1.0));
    let AnswerValue::Score {
        expected_level,
        probabilities,
        ..
    } = &answers[2].value
    else {
        panic!();
    };
    assert!(approx(*expected_level, 3.0));
    assert!(approx(probabilities[3], 0.5));

    let body: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(body["model"], model);
    assert_eq!(body["response_format"], json!({ "type": "json_object" }));
    let prompt = body["messages"][1]["content"].as_str().unwrap();
    for needle in ["材料", "\"addressed\"", "\"revise\"", "很好", "answers"] {
        assert!(prompt.contains(needle), "prompt lacks {needle}: {prompt}");
    }
}

#[tokio::test]
async fn decide_via_chat_accepts_bare_answer_map_and_reports_garbage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "<think>想想</think>{\"addressed\": {\"noul\": 0.2}}" } }]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "抱歉，我无法回答。" } }]
        })))
        .mount(&server)
        .await;
    let p = Provider::new(ProviderKind::OpenAiCompatible, server.uri());
    let profile = resolve_profile("gpt-4o", None, None);
    let qs = &questions()[..1];
    let answers = client()
        .decide_via_chat(&p, "gpt-4o", &profile, "s", qs)
        .await
        .unwrap();
    assert_eq!(answers[0].value, AnswerValue::YesNo { p_yes: 0.2 });
    let err = client()
        .decide_via_chat(&p, "gpt-4o", &profile, "s", qs)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}
