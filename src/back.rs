use chrono::prelude::*;
use erase_output::Erase;
use futures::{
    future::{join_all, Either},
    stream, FutureExt, StreamExt,
};
use mmagolf::{
    codetest, connect_to_server, display_compile_error, submit, Command, ReternMessage, Submission,
    SubmissionResultType,
};
use serde_json::json;
use slack_hook::{PayloadBuilder, Slack};
use std::{
    collections::HashMap,
    fmt::Display,
    io::Write,
    iter,
    net::TcpStream,
    path::{Path, PathBuf},
    process::exit,
};
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
            let (_, result, (problems, new_submission_id, mut file, language_shortests)) =
                futures::join!(submission, display_result, submission_list);
            let new_submission = &Submission {
                id: new_submission_id,
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
                let s_str = format!("{}\n", new_submission);
                let write1 = file.write_all(s_str.as_bytes());
                let write2 = save_submission(&code, new_submission_id);
                let (position, submissions) = insert_submission(problems, new_submission.clone());
                let mut is_language_shortest = false;
                let write3 = if language_shortests
                    .get(&(
                        new_submission.problem.to_string(),
                        new_submission.lang.clone(),
                    ))
                    .map(|&shortest| new_submission.size < shortest)
                    .unwrap_or(true)
                {
                    is_language_shortest = true;
                    let submitted_files = SubmittedFiles::new(new_submission_id, code.clone());
                    Either::Left(make_ranking(
                        &submissions,
                        position,
                        submitted_files,
                        FileSender::new(),
                    ))
                } else {
                    Either::Right(async {})
                };
                let (a, _, _) = futures::join!(write1, write2, write3);
                a.unwrap();
                match submissions[problem_number - 1]
                    .get(0)
                    .map(|shortest| new_submission.id == shortest.id)
                {
                    None | Some(true) => shortest(new_submission, &code),
                    _ if is_language_shortest => {
                        println!("Shortest code in {}! 🎉", new_submission.lang)
                    }
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
    Re(u64),
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
            JudgeStatus::Re(t) => write!(
                f,
                "{}{}RE{}{}  {t: >7} ms",
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
    let mut re = false;
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
            JudgeStatus::Re(t) => {
                re = true;
                time = time.max(t);
            }
            JudgeStatus::Wj => panic!(),
        }
    }
    if tle {
        JudgeStatus::Tle(time)
    } else if re {
        JudgeStatus::Re(time)
    } else if wa {
        JudgeStatus::Wa(time)
    } else if ac {
        JudgeStatus::Ac(time)
    } else {
        panic!()
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
                } else {
                    match result {
                        SubmissionResultType::Ac => JudgeStatus::Ac(time),
                        SubmissionResultType::Re => JudgeStatus::Re(time),
                        SubmissionResultType::Wa => JudgeStatus::Wa(time),
                    }
                };
            }
            Ok(ReternMessage::Close) => {
                break;
            }
            Ok(ReternMessage::CompileError {
                code,
                stdout,
                stderr,
            }) => {
                print!("{}", Erase(&old));
                display_compile_error(code, stdout, stderr).await;
                return None;
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

async fn get_submission_list() -> (
    Vec<Vec<Submission>>,
    usize,
    File,
    HashMap<(String, String), usize>,
) {
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
    let mut language_shortest: HashMap<(String, String), usize> = HashMap::new();
    let submissions: Vec<_> = s.lines().collect();
    let total_submission_number = submissions.len();
    let mut submissions: Vec<_> = submissions
        .into_iter()
        .enumerate()
        .map(|(i, l)| Submission::from_str(l, i).unwrap())
        .filter(|submission| {
            let shortest = language_shortest
                .get(&(submission.problem.to_string(), submission.lang.clone()))
                .copied()
                .unwrap_or(usize::MAX);
            if submission.size < shortest {
                language_shortest.insert(
                    (submission.problem.to_string(), submission.lang.clone()),
                    submission.size,
                );
                true
            } else {
                false
            }
        })
        .collect();
    submissions.sort_unstable_by_key(|s| (s.size, s.id));
    let mut problems = vec![Vec::new(); NUMBER_OF_PROBLEMS];
    for s in submissions {
        problems[s.problem - 1].push(s);
    }
    (problems, total_submission_number, file, language_shortest)
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

fn insert_submission(
    mut submissions: Vec<Vec<Submission>>,
    new_submission: Submission,
) -> (usize, Vec<Vec<Submission>>) {
    let i =
        submissions[new_submission.problem - 1].partition_point(|x| x.size <= new_submission.size);
    submissions[new_submission.problem - 1].insert(i, new_submission);
    (i, submissions)
}

struct SubmittedFiles {
    path: PathBuf,
    catch: HashMap<usize, String>,
}

impl SubmittedFiles {
    fn new(new_submission_id: usize, new_submission_code: String) -> SubmittedFiles {
        let mut catch = HashMap::new();
        catch.insert(new_submission_id, new_submission_code);
        SubmittedFiles {
            path: std::path::Path::new(HOME_DIR).join(".local/share/mmagolf/submitted_files"),
            catch,
        }
    }

    async fn get(&mut self, id: usize) -> &str {
        if self.catch.contains_key(&id) {
            &self.catch[&id]
        } else {
            let code = fs::read_to_string(self.path.join(id.to_string()))
                .await
                .unwrap();
            self.catch.insert(id, code);
            &self.catch[&id]
        }
    }

    fn get_from_catch(&self, id: usize) -> Option<&str> {
        self.catch.get(&id).map(|s| &s[..])
    }
}

struct FileSender {
    sesstion: ssh2::Session,
}

impl FileSender {
    fn new() -> Self {
        let tcp = TcpStream::connect("webserver.lxd.saga.mma.club.uec.ac.jp:22").unwrap();
        let mut sesstion = ssh2::Session::new().unwrap();
        sesstion.set_tcp_stream(tcp);
        sesstion.handshake().unwrap();
        sesstion
            .userauth_pubkey_file(
                "mado",
                None,
                &Path::new(HOME_DIR).join(".ssh/id_ed25519_web"),
                None,
            )
            .unwrap();
        FileSender { sesstion }
    }

    fn send(&self, remote_path: &Path, contents: String) {
        let mut f = self
            .sesstion
            .scp_send(remote_path, 0o644, contents.len() as u64, None)
            .unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.send_eof().unwrap();
        f.wait_eof().unwrap();
        f.close().unwrap();
        f.wait_close().unwrap();
    }
}

async fn make_ranking(
    submissions: &[Vec<Submission>],
    new_submission_rank: usize,
    submitted_files: SubmittedFiles,
    file_sender: FileSender,
) {
    if new_submission_rank >= RANK_LEN {
        return;
    }
    let submitted_files = stream::iter(submissions.iter().flatten())
        .fold(submitted_files, |mut f, s| async {
            f.get(s.id).await;
            f
        })
        .await;
    let s = json!(
        join_all(
            submissions
                .iter()
                .map(|p| join_all(p.iter().take(RANK_LEN).map(|s| async {
                    let code = submitted_files.get_from_catch(s.id).unwrap();
                    let code = htmlescape::encode_minimal(code);
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
    println!("{}", s);
    #[cfg(not(feature = "localhost_server"))]
    file_sender.send(Path::new("/home/mado/public_html/golf/ranking.json"), s);
}

#[cfg(not(feature = "localhost_server"))]
const WEBHOOK_URL: &str = include_str!("webhook_url");

fn shortest(submission: &Submission, code: &str) {
    #[cfg(not(feature = "localhost_server"))]
    {
        let slack = Slack::new(WEBHOOK_URL).unwrap();
        let p = PayloadBuilder::new()
            .text(format!(
                "{}が{}で問題{}のShortestを更新しました！（{} B）\n```{}```",
                submission.user, submission.lang, submission.problem, submission.size, code
            ))
            .username("Shortest更新通知")
            .icon_emoji(":golf:")
            .channel("#_mado")
            .build()
            .unwrap();
        slack.send(&p).unwrap();
    }
    println!("Shortest! 🎉");
}
