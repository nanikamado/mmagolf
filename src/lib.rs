use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::mpsc::Sender};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, protocol::Message},
    MaybeTlsStream, WebSocketStream,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all(deserialize = "snake_case"))]
pub enum ReternMessage {
    SubmissionResult {
        test_case_number: usize,
        result: bool,
        time: u64,
        killed: bool,
    },
    CodetestResult {
        result: Vec<u8>,
        time: u64,
    },
    NumberOfTestCases {
        n: usize,
    },
    Close,
    NotSuchProblem {
        problem_number: usize,
    },
    NotSuchLang {
        lang: String,
    },
}

pub async fn submit(
    lang: String,
    problem_number: usize,
    code: String,
    mut ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    sender: Sender<ReternMessage>,
) {
    ws_stream
        .send(Message::Text(
            json!({
                "type": "submission",
                "lang": lang,
                "problem_number": problem_number,
                "code": code,
            })
            .to_string(),
        ))
        .await
        .unwrap();
    ws_stream
        .for_each(|message| async {
            match message.unwrap() {
                Message::Text(message) => {
                    let data = serde_json::from_str(&message).unwrap();
                    sender.send(data).await.unwrap();
                }
                Message::Close(_) => {
                    let _ = sender.send(ReternMessage::Close).await;
                }
                _ => panic!(),
            }
        })
        .await;
}

pub async fn codetest(
    lang: String,
    code: String,
    input: Option<Vec<u8>>,
    mut ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
) {
    ws_stream
        .send(Message::Text(
            json!({
                "type": "codetest",
                "lang": lang,
                "code": code,
                "input" : input.map(base64::encode),
            })
            .to_string(),
        ))
        .await
        .unwrap();
    ws_stream
        .for_each(|message| async {
            let message = message.unwrap();
            if let Message::Text(data) = message {
                let v: serde_json::map::Map<_, _> = serde_json::from_str(&data).unwrap();
                tokio::io::stdout()
                    .write_all(&base64::decode(v["result"].as_str().unwrap()).unwrap())
                    .await
                    .unwrap();
            }
        })
        .await;
}

pub async fn connect_to_server(
    server_address: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Error> {
    let url = url::Url::parse(&format!("ws://{}:5620", server_address)).unwrap();
    Ok(connect_async(url).await?.0)
}
