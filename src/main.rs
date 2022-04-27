use chrono::prelude::*;
use clap::{Parser, Subcommand};
use erase_output::Erase;
use mmagolf::{codetest, connect_to_server, submit, ReternMessage, Submission};
use std::{fmt::Display, fs::read_to_string, io::Read, iter, process::exit};
use termion::{color, style};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::{channel, Receiver},
};
use users::{get_current_uid, get_user_by_uid};

#[derive(Debug, Parser)]
#[clap(version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Submit code
    #[clap(arg_required_else_help = true)]
    Submit {
        #[clap(short, long, value_name = "FILE")]
        code: String,
        #[clap(short, long)]
        lang: String,
        #[clap(short, long)]
        problem_number: usize,
    },
    /// Run the code in the judge surver to see if the code works
    #[clap(
        arg_required_else_help = true,
        override_usage = "echo <INPUT> | mmagolf codetest --code <CODE> --lang <LANG>"
    )]
    Codetest {
        /// Source code
        #[clap(short, long, value_name = "FILE")]
        code: String,
        /// Language
        #[clap(short, long)]
        lang: String,
    },
}

#[cfg(not(feature = "localhost_server"))]
const SERVER_ADDRESS: &str = "atlas";
#[cfg(feature = "localhost_server")]
const SERVER_ADDRESS: &str = "localhost";

#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let ws_stream = connect_to_server(SERVER_ADDRESS).await.unwrap_or_else(|e| {
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
    });
    match args.command {
        Commands::Submit {
            code,
            lang,
            problem_number,
        } => {
            let submission_list = get_submission_list();
            let code = read_to_string(&code).unwrap_or_else(|e| {
                eprintln!("{}: {}", code, e);
                exit(1)
            });
            let s = &Submission {
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
            let (sender, receiver) = channel(100);
            let submission = submit(&s.lang, problem_number, &code, ws_stream, sender);
            let display_result = display_result(receiver, s.size);
            let (_, result, (problems, n, mut file)) =
                futures::join!(submission, display_result, submission_list);
            if matches!(result, Some(JudgeStatus::Ac(_))) {
                file.write_all(format!("{}\n", s).as_bytes()).await.unwrap();
                save_submission(&code, n).await;
                match problems[problem_number - 1]
                    .get(0)
                    .map(|shortest| s.size < shortest.size)
                {
                    None | Some(true) => println!("Shortest! 🎉"),
                    _ => (),
                }
            }
        }
        Commands::Codetest { code, lang } => {
            let code = read_to_string(&code).unwrap_or_else(|e| {
                eprintln!("{}: {}", code, e);
                exit(1)
            });
            let input = if atty::is(atty::Stream::Stdin) {
                None
            } else {
                Some({
                    let mut input = Vec::new();
                    std::io::stdin()
                        .read_to_end(&mut input)
                        .unwrap_or_else(|e| {
                            eprintln!("{e}");
                            exit(1)
                        });
                    input
                })
            };
            codetest(lang, code, input, ws_stream).await;
        }
    }
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
        .map(|l| Submission::from_str(l).unwrap())
        .collect();
    let total_submission_number = submissions.len();
    submissions.sort_by_key(|s| s.size);
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
