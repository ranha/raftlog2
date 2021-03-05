# バグを疑っている箇所

* `cluster.rs/median`及びそれを使っている箇所

# Raftの解説

Core raftとメンバ構成変更に分けて説明する。

## その前に先に念頭に入れておくべきこと
CAP schemaでいうところのC(onsistency)とP(artial torelance)重視なので
A(vailability)が欠けてしまう。

システムと切り離された外部からは応答不能状態、すなわち
システムが「全く動いていない」（実際は内部で忙しくしていても）
ように見えることがある。

## Core Raft
計算は2つの要素から構成される:
1. Leaderを選出すること
2. LeaderからReplicaにデータを送りつけること。

MultiPaxosと似ていないというのは難しいので、
仕方なく似ていると言うことにするが、
ReplicaからLeaderにデータを送りつけることがない決定的な違いある。

したがって、過半数票を集まれば乗っ取れるMultiPaxosとは違い、
Leaderは「過半数表を得る」ことに加えて「資質」を満たしている必要がある。

資質とは、最後のtermに本質的に参加していた過半数いるはずのメンバーであること。
過半数のうちで最もデータを蓄えているものである必要はない。
従ってある種のRoll Backが起こっている可能性があるが、
これはcommitしていないからと思うことにすれば問題ない。

## メンバ構成変更
ただし、資質を満たす上ではメンバ構成変更について注意する必要がある。
裏で勝手にメンバが増えてしまった場合には、資質が絶対に満たされなくなるからである。


# RaftLogは何を実装しているか
* Core raft
* メンバ構成変更

オリジナルの論文にない工夫について。

## Implementation
```
src
├── cluster.rs (Raftクラスタに関するコード群)
├── election.rs 選挙のための基本構造 (Term, Ballot, Role)
├── error.rs (このライブラリのエラー表現を束ねるもの)
├── io.rs (I/O処理を抽象化したtrait; I/O処理はストレージI/OとネットワークI/Oの二種類)
├── lib.rs (このライブラリを総括するモジュール)
├── log
│   ├── history.rs（ログではないhistoryという謎の構造を定義している）
│   └── mod.rs（ログを定義しているが複雑すぎる）
├── message.rs（メッセージパッシング用の構造体など）
├── metrics.rs（Prometheusなどで使うためのもので計算には無関係）
├── node.rs（Raft clusterの基本構成単位であるノードを表す構造体等）
├── node_state（各nodeでの計算を行うためのもの。基本的に複雑すぎる）
│   ├── candidate.rs
│   ├── common
│   │   ├── mod.rs
│   │   └── rpc_builder.rs
│   ├── follower
│   │   ├── append.rs
│   │   ├── idle.rs
│   │   ├── init.rs
│   │   ├── mod.rs
│   │   └── snapshot.rs
│   ├── leader
│   │   ├── appender.rs
│   │   ├── follower.rs
│   │   └── mod.rs
│   ├── loader.rs
│   └── mod.rs
├── replicated_log.rs（命名がかなり悪い。これはraft clusterそのものまたはreplicated state machineのこと）
└── test_util.rs
```
