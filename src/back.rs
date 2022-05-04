use chrono::prelude::*;
use erase_output::Erase;
use futures::{future::join_all, FutureExt};
use mmagolf::{codetest, connect_to_server, submit, Command, ReternMessage, Submission};
use serde_json::json;
use ssh2::Session;
use std::{fmt::Display, io::Write, iter, net::TcpStream, path::Path, process::exit};
use termion::{color, style};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::{channel, Receiver},
};
use users::{get_current_uid, get_user_by_uid};

#[cfg(not(feature = "localhost_server"))]
const SERVER_ADDRESS: &str = "atlas";
#[cfg(feature = "localhost_server")]
const SERVER_ADDRESS: &str = "localhost";

#[tokio::main]
async fn main() {
    let input = read_input();
    let ws_stream = connect_to_server(SERVER_ADDRESS).map(|s| {
        s.unwrap_or_else(|e| {
            use tokio_tungstenite::tungstenite::Error::*;
            match e {
                Io(e) => {
                    eprintln!(
                        "ジャッジサーバーに接続できませんでした。\
                ジャッジサーバーが動いていないかもしれません。{}",
                        e
                    );
                    exit(1);
                }
                _ => {
                    eprintln!(
                        "ジャッジサーバーに接続できませんでした。原因はよくわかりません。:{}",
                        e
                    );
                    exit(1);
                }
            }
        })
    });
    let (input, ws_stream) = tokio::join!(input, ws_stream);
    match input {
        Command::Submit {
            code,
            lang,
            problem_number,
        } => {
            let submission_list = get_submission_list();
            let (sender, receiver) = channel(100);
            let submission = submit(&lang, problem_number, &code, ws_stream, sender);
            let display_result = display_result(receiver, code.len());
            let (_, result, (problems, n, mut file)) =
                futures::join!(submission, display_result, submission_list);
            let s = &Submission {
                id: n,
                size: code.len(),
                problem: problem_number,
                lang,
                time: Utc::now(),
                user: get_user_by_uid(get_current_uid())
                    .unwrap()
                    .name()
                    .to_string_lossy()
                    .to_string(),
            };
            if matches!(result, Some(JudgeStatus::Ac(_))) {
                let s_str = format!("{}\n", s);
                let write1 = file.write_all(s_str.as_bytes());
                let write2 = save_submission(&code, n);
                let write3 = make_ranking(&code, problems.clone(), s.clone());
                let (a, _, _) = futures::join!(write1, write2, write3);
                a.unwrap();
                match problems[problem_number - 1]
                    .get(0)
                    .map(|shortest| s.size < shortest.size)
                {
                    None | Some(true) => println!("Shortest! 🎉"),
                    _ => (),
                }
            }
        }
        Command::Codetest { code, lang, input } => {
            codetest(
                lang,
                code,
                input.map(|i| base64::decode(i).unwrap()),
                ws_stream,
            )
            .await;
        }
    }
}

async fn read_input() -> Command {
    let mut input = String::new();
    tokio::io::stdin().read_to_string(&mut input).await.unwrap();
    serde_json::from_str(&input).unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(unused)]
enum JudgeStatus {
    Ac(u64),
    // Mle(u64),
    Tle(u64),
    // Re,
    // Ole,
    // Ie,
    Wa(u64),
    Wj,
}

impl Display for JudgeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JudgeStatus::Ac(t) => write!(
                f,
                "{}{}AC{}{}  {t: >7} ms",
                style::Bold,
                color::Fg(color::Green),
                color::Fg(color::Reset),
                style::Reset,
            ),
            JudgeStatus::Tle(t) => write!(
                f,
                "{}{}TLE{}{}  {t: >7} ms",
                style::Bold,
                color::Fg(color::Yellow),
                color::Fg(color::Reset),
                style::Reset,
            ),
            JudgeStatus::Wa(t) => write!(
                f,
                "{}{}WA{}{}  {t: >7} ms",
                style::Bold,
                color::Fg(color::Yellow),
                color::Fg(color::Reset),
                style::Reset,
            ),
            JudgeStatus::Wj => write!(f, "..."),
        }
    }
}

fn statuses_to_string(judge_statuses: &[JudgeStatus], n: usize) -> String {
    judge_statuses
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if *s == JudgeStatus::Wj {
                format!(
                    "Test Case {}: {}\n",
                    i + 1,
                    iter::once(".").cycle().take(n).collect::<String>()
                )
            } else {
                format!("Test Case {}: {s}\n", i + 1)
            }
        })
        .collect()
}

fn overall_result(judge_statuses: &[JudgeStatus]) -> JudgeStatus {
    let mut ac = false;
    let mut tle = false;
    let mut wa = false;
    let mut time = 0;
    for s in judge_statuses {
        match *s {
            JudgeStatus::Ac(t) => {
                ac = true;
                time = time.max(t);
            }
            JudgeStatus::Tle(t) => {
                tle = true;
                time = time.max(t);
            }
            JudgeStatus::Wa(t) => {
                wa = true;
                time = time.max(t);
            }
            JudgeStatus::Wj => panic!(),
        }
    }
    match (ac, tle, wa) {
        (_, _, true) => JudgeStatus::Wa(time),
        (_, true, false) => JudgeStatus::Tle(time),
        (true, false, false) => JudgeStatus::Ac(time),
        (false, false, false) => panic!(),
    }
}

async fn display_result(mut receiver: Receiver<ReternMessage>, size: usize) -> Option<JudgeStatus> {
    let number_of_cases = match receiver.recv().await {
        Some(ReternMessage::NumberOfTestCases { n }) => n,
        Some(ReternMessage::NotSuchProblem { problem_number }) => {
            println!("Not such problem: {problem_number}");
            return None;
        }
        Some(ReternMessage::NotSuchLang { lang }) => {
            println!("Not such language: {lang}");
            return None;
        }
        r => panic!("{:?}", r),
    };
    let mut judge_statuses = vec![JudgeStatus::Wj; number_of_cases];
    let mut old = String::new();
    for i in (0..4).cycle() {
        match receiver.try_recv() {
            Ok(ReternMessage::SubmissionResult {
                test_case_number,
                result,
                time,
                killed,
            }) => {
                judge_statuses[test_case_number] = if killed {
                    JudgeStatus::Tle(time)
                } else if result {
                    JudgeStatus::Ac(time)
                } else {
                    JudgeStatus::Wa(time)
                };
            }
            Ok(ReternMessage::Close) => {
                break;
            }
            _ => (),
        }
        let s = statuses_to_string(&judge_statuses, i);
        print!("{}{}", Erase(&old), s);
        old = s;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let result = overall_result(&judge_statuses);
    println!("\nResult: {}, {} B", result, size);
    Some(result)
}

const NUMBER_OF_PROBLEMS: usize = 3;
const HOME_DIR: &str = env!("HOME");

async fn get_submission_list() -> (Vec<Vec<Submission>>, usize, File) {
    let data_dir = std::path::Path::new(HOME_DIR).join(".local/share/mmagolf");
    fs::create_dir_all(&data_dir).await.unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .read(true)
        .create(true)
        .open(data_dir.join("submissions"))
        .await
        .unwrap();
    let mut s = String::new();
    file.read_to_string(&mut s).await.unwrap();
    let mut submissions: Vec<_> = s
        .lines()
        .enumerate()
        .map(|(i, l)| Submission::from_str(l, i).unwrap())
        .collect();
    let total_submission_number = submissions.len();
    submissions.sort_unstable_by_key(|s| (s.size, s.id));
    let mut problems = vec![Vec::new(); NUMBER_OF_PROBLEMS];
    for s in submissions {
        problems[s.problem - 1].push(s);
    }
    (problems, total_submission_number, file)
}

async fn save_submission(code: &str, n: usize) {
    let submitted_files =
        std::path::Path::new(HOME_DIR).join(".local/share/mmagolf/submitted_files");
    fs::create_dir_all(&submitted_files).await.unwrap();
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(submitted_files.join(n.to_string()))
        .await
        .unwrap();
    file.write_all(code.as_bytes()).await.unwrap();
}

const RANK_LEN: usize = 10;

async fn make_ranking(
    code: &str,
    mut submissions: Vec<Vec<Submission>>,
    new_submission: Submission,
) {
    let i =
        submissions[new_submission.problem - 1].partition_point(|x| x.size <= new_submission.size);
    if i >= RANK_LEN {
        return;
    }
    let submitted_files =
        std::path::Path::new(HOME_DIR).join(".local/share/mmagolf/submitted_files");
    let id = new_submission.id;
    submissions[new_submission.problem - 1].insert(i, new_submission);
    let s = json!(
        join_all(
            submissions
                .iter()
                .map(|p| join_all(p.iter().take(RANK_LEN).map(|s| async {
                    let code = if s.id == id {
                        code.to_string()
                    } else {
                        fs::read_to_string(submitted_files.join(s.id.to_string()))
                            .await
                            .unwrap()
                    };
                    let code = htmlescape::encode_minimal(&code);
                    let time: DateTime<Local> = DateTime::from(s.time);
                    [
                        s.size.to_string(),
                        s.lang.clone(),
                        s.user.clone(),
                        time.format("%Y-%m-%d %H:%M:%S").to_string(),
                        code,
                    ]
                })))
        )
        .await
    )
    .to_string();
    #[cfg(feature = "localhost_server")]
    {
        println!("{}", s);
    }
    #[cfg(not(feature = "localhost_server"))]
    {
        let tcp = TcpStream::connect("webserver.lxd.saga.mma.club.uec.ac.jp:22").unwrap();
        let mut sess = Session::new().unwrap();
        sess.set_tcp_stream(tcp);
        sess.handshake().unwrap();
        sess.userauth_pubkey_file(
            "mado",
            None,
            &Path::new(HOME_DIR).join(".ssh/id_ed25519_web"),
            None,
        )
        .unwrap();
        // Write the file
        let mut ranking_json = sess
            .scp_send(
                Path::new("/home/mado/public_html/golf/ranking.json"),
                0o644,
                s.len() as u64,
                None,
            )
            .unwrap();
        ranking_json.write_all(s.as_bytes()).unwrap();
        ranking_json.send_eof().unwrap();
        ranking_json.wait_eof().unwrap();
        // Close the channel and wait for the whole content to be tranferred
        ranking_json.close().unwrap();
        ranking_json.wait_close().unwrap();
    }
}
