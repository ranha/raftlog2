use futures::{Async, Future, Poll};
use raftlog::election::{Ballot, Role};
use raftlog::log::{Log, LogIndex, LogPrefix, LogSuffix};
use raftlog::message::Message;
use raftlog::node::NodeId;
use raftlog::{Error, Io, Result};
use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/*
 * Mock用のIO
 */
pub struct MockIo {
    node_id: NodeId,
    recv: Receiver<Message>,
    send: Sender<Message>, // recvにデータを送るためのsender; コピー可能なので他のノードにコピーして渡す
    channels: BTreeMap<NodeId, Sender<Message>>,
    ballots: Vec<Ballot>,
    snapshotted: Option<LogPrefix>,
    rawlogs: Option<LogSuffix>,
    candidate_invoker: Option<Sender<()>>,
    follower_invoker: Option<Sender<()>>,
    leader_invoker: Option<Sender<()>>,
}

impl MockIo {
    pub fn new(node_id: &str) -> Self {
        let (send, recv) = channel();
        Self {
            node_id: NodeId::new(node_id),
            recv,
            send,
            channels: Default::default(),
            ballots: Vec::new(),
            snapshotted: None,
            rawlogs: None,
            candidate_invoker: None,
            follower_invoker: None,
            leader_invoker: None,
        }
    }

    pub fn copy_sender(&self) -> Sender<Message> {
        self.send.clone()
    }
}

impl Io for MockIo {
    fn try_recv_message(&mut self) -> Result<Option<Message>> {
        let r = self.recv.try_recv();
        if let Ok(v) = r {
            Ok(Some(v))
        } else {
            let err = r.err().unwrap();
            if err == TryRecvError::Empty {
                Ok(None)
            } else {
                panic!("disconnected");
            }
        }
    }

    fn send_message(&mut self, message: Message) {
        let dest: NodeId = message.header().destination.clone();
        let channel = self.channels.get(&dest).unwrap();
        channel.send(message).unwrap();
    }

    type SaveBallot = BallotSaver;
    fn save_ballot(&mut self, ballot: Ballot) -> Self::SaveBallot {
        self.ballots.push(ballot.clone());
        BallotSaver(self.node_id.clone(), ballot)
    }

    // 最後に行った投票状況を取り戻す
    // （それとも、最後にsaveしたものを取り戻す?）
    type LoadBallot = BallotLoader;
    fn load_ballot(&mut self) -> Self::LoadBallot {
        let last_pos = self.ballots.len() - 1;
        let ballot = self.ballots[last_pos].clone();
        BallotLoader(self.node_id.clone(), ballot)
    }

    type SaveLog = LogSaver;
    // prefixは必ず前回部分を含んでいるものか？
    // それとも新しい差分か？ 多分前者だと思うが何も分からない
    fn save_log_prefix(&mut self, prefix: LogPrefix) -> Self::SaveLog {
        if let Some(snap) = &self.snapshotted {
            // 保存するデータはcommit済みのハズなので
            // この辺は問題ないだろうsnapshotについてはこれが成立するはず
            assert!(prefix.tail.is_newer_or_equal_than(snap.tail));
        }
        self.snapshotted = Some(prefix);
        LogSaver(SaveMode::SnapshotSave, self.node_id.clone())
    }
    fn save_log_suffix(&mut self, suffix: &LogSuffix) -> Self::SaveLog {
        if let Some(rawlogs) = &self.rawlogs {
            // これもcommit済みのデータが来るはずなので通ると思う
            assert!(suffix.head.is_newer_or_equal_than(rawlogs.head));
        }
        self.rawlogs = Some(suffix.clone());
        LogSaver(SaveMode::RawLogSave, self.node_id.clone())
    }

    type LoadLog = LogLoader;
    fn load_log(&mut self, start: LogIndex, end: Option<LogIndex>) -> Self::LoadLog {
        // 要求領域がoverlapしていないことを網羅的に検査すること
        if end == None {
            if let Some(snap) = &self.snapshotted {
                if self.rawlogs.is_some() {
                    // snap ++ rawlogs を 表現するすべがない
                    println!("load: error 1");
                } else {
                    // only snapshot
                    if snap.is_match(start, end) {
                        // we have only a snapshot
                        return LogLoader(Some(Log::from(snap.clone())));
                    }
                }
            } else {
                if let Some(rawlogs) = &self.rawlogs {
                    let mut rawlogs = rawlogs.clone();
                    rawlogs.skip_to(start).unwrap();
                    return LogLoader(Some(Log::from(rawlogs)));
                }
            }
        }

        let end = end.unwrap();
        assert!(start < end);

        if let Some(snap) = &self.snapshotted {
            if snap.is_match(start, Some(end)) {
                return LogLoader(Some(Log::from(snap.clone())));
            }
        }

        if let Some(rawlogs) = &self.rawlogs {
            if let Ok(sliced) = rawlogs.slice(start, end) {
                return LogLoader(Some(Log::from(sliced)));
            }
        }

        panic!("start = {:?}, end = {:?}", start, end);
    }

    type Timeout = Invoker;
    fn create_timeout(&mut self, role: Role) -> Self::Timeout {
        self.candidate_invoker = None;
        self.follower_invoker = None;
        self.leader_invoker = None;

        let (invoker, timer) = channel();
        match role {
            Role::Candidate => {
                self.candidate_invoker = Some(invoker);
            }
            Role::Follower => {
                self.follower_invoker = Some(invoker);
            }
            Role::Leader => {
                self.leader_invoker = Some(invoker);
            }
        }
        Invoker(timer)
    }
}

pub struct Invoker(Receiver<()>);

impl Future for Invoker {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let r = self.0.try_recv();
        if r.is_ok() {
            Ok(Async::Ready(()))
        } else {
            let err = r.err().unwrap();
            if err == TryRecvError::Empty {
                Ok(Async::NotReady)
            } else {
                panic!("disconnected");
            }
        }
    }
}

pub struct LogLoader(Option<Log>);
impl Future for LogLoader {
    type Item = Log;
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let Some(log) = self.0.take() {
            Ok(Async::Ready(log))
        } else {
            panic!("")
        }
    }
}

enum SaveMode {
    SnapshotSave,
    RawLogSave,
}
pub struct LogSaver(SaveMode, pub NodeId);
impl Future for LogSaver {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0 {
            SaveMode::SnapshotSave => {
                println!("[Node {}] Save Snapshot", self.1.as_str())
            }
            SaveMode::RawLogSave => {
                println!("[Node {}] Save Raw Logs", self.1.as_str())
            }
        }
        Ok(Async::Ready(()))
    }
}

pub struct BallotSaver(pub NodeId, pub Ballot);
pub struct BallotLoader(pub NodeId, pub Ballot);

impl Future for BallotSaver {
    type Item = ();
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        println!(
            "[Node {}] Save Ballot({})",
            self.0.as_str(),
            self.1.to_str()
        );
        Ok(Async::Ready(()))
    }
}

impl Future for BallotLoader {
    type Item = Option<Ballot>;
    type Error = Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        println!(
            "[Node {}] Load Ballot({})",
            self.0.as_str(),
            self.1.to_str()
        );
        Ok(Async::Ready(Some(self.1.clone())))
    }
}

fn main() {
    println!("Hello, World!");
}
