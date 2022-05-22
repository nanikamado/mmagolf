use chrono::prelude::*;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fmt::Display, os::unix::prelude::ExitStatusExt};
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::mpsc::Sender};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, protocol::Message},
    MaybeTlsStream, WebSocketStream,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Submit {
        code: String,
        lang: String,
        problem_number: usize,
    },
    Codetest {
        code: String,
        lang: String,
        input: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all(deserialize = "snake_case"))]
pub enum ReternMessage {
    SubmissionResult {
        test_case_number: usize,
        result: SubmissionResultType,
        time: u64,
        killed: bool,
    },
    CompileError {
        code: i32,
        stdout: String,
        stderr: String,
    },
    CodetestResult {
        stdout: String,
        stderr: String,
        time: u64,
        killed: bool,
        status: Option<i32>,
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

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all(deserialize = "snake_case"))]
pub enum SubmissionResultType {
    Ac,
    Re,
    Wa,
}

pub async fn submit(
    lang: &str,
    problem_number: usize,
    code: &str,
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
            if let Message::Text(message) = message {
                let data: ReternMessage = serde_json::from_str(&message).unwrap();
                match data {
                    ReternMessage::CodetestResult {
                        stdout,
                        time,
                        stderr,
                        killed,
                        status,
                    } => {
                        if killed {
                            println!("TLEです。");
                        }
                        println!("time: {time} ms\nresult:");
                        if let Some(status) = status {
                            println!("{}", std::process::ExitStatus::from_raw(status));
                        }
                        tokio::io::stdout()
                            .write_all(&base64::decode(stdout).unwrap())
                            .await
                            .unwrap();
                        tokio::io::stderr()
                            .write_all(&base64::decode(stderr).unwrap())
                            .await
                            .unwrap();
                    }
                    ReternMessage::NotSuchLang { lang } => {
                        println!("Not such language: {lang}");
                    }
                    ReternMessage::CompileError {
                        code,
                        stdout,
                        stderr,
                    } => display_compile_error(code, stdout, stderr).await,
                    _ => panic!("{:?}", data),
                }
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

pub async fn display_compile_error(code: i32, stdout: String, stderr: String) {
    let mut output = format!("Result: Compile Error\nexit code: {}\nstdout:\n", code).into_bytes();
    output.append(&mut base64::decode(stdout).unwrap());
    output.append(&mut "stderr:\n".as_bytes().to_vec());
    output.append(&mut base64::decode(stderr).unwrap());
    tokio::io::stdout().write_all(&output).await.unwrap();
}

#[derive(Debug, Clone)]
pub struct Submission {
    pub id: usize,
    pub size: usize,
    pub problem: usize,
    pub lang: String,
    pub time: DateTime<Utc>,
    pub user: String,
}

impl Submission {
    pub fn from_str(s: &str, n: usize) -> Option<Self> {
        let mut s = s.split_whitespace();
        let size = s.next()?.parse::<usize>().ok()?;
        let problem = s.next()?.parse::<usize>().ok()?;
        let lang = s.next()?.to_string();
        let time = Utc.timestamp(s.next()?.parse::<i64>().ok()?, 0);
        let user = s.next()?.to_string();
        Some(Submission {
            id: n,
            size,
            problem,
            lang,
            time,
            user,
        })
    }
}

impl Display for Submission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {}",
            self.size,
            self.problem,
            self.lang,
            self.time.timestamp(),
            self.user
        )
    }
}
