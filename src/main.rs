use clap::{Parser, Subcommand};
use erase_output::Erase;
use futures_util::future;
use mmagolf::{codetest, connect_to_server, submit, ReternMessage};
use std::{fmt::Display, fs::read_to_string, io::Read, iter, process::exit};
use termion::{color, style};
use tokio::sync::mpsc::{channel, Receiver};

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

#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let server_address = std::env::var("MMA_GOLF_SERVER").unwrap_or("atlas".to_string());
    match args.command {
        Commands::Submit {
            code,
            lang,
            problem_number,
        } => {
            let code = read_to_string(&mut code.clone()).unwrap_or_else(|e| {
                eprintln!("{}: {}", code, e);
                exit(1)
            });
            let ws_stream = connect_to_server(&server_address);
            let (sender, receiver) = channel(100);
            let submission = submit(lang, problem_number, code, ws_stream.await, sender);
            let display_result = display_result(receiver);
            future::join(submission, display_result).await;
        }
        Commands::Codetest { code, lang } => {
            let code = read_to_string(&mut code.clone()).unwrap_or_else(|e| {
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
            let ws_stream = connect_to_server(&server_address).await;
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

impl JudgeStatus {
    fn to_string_with_emoji(&self) -> String {
        match self {
            &JudgeStatus::Ac(t) => format!("{} 🎉", JudgeStatus::Ac(t)),
            s => format!("{s}"),
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

async fn display_result(mut receiver: Receiver<ReternMessage>) {
    let number_of_cases = match receiver.recv().await {
        Some(ReternMessage::NumberOfTestCases { n }) => n,
        Some(ReternMessage::NotSuchProblem { problem_number }) => {
            println!("Not such problem: {problem_number}");
            return;
        }
        Some(ReternMessage::NotSuchLang { lang }) => {
            println!("Not such language: {lang}");
            return;
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
    println!(
        "\nResult: {}",
        overall_result(&judge_statuses).to_string_with_emoji()
    );
}
