//! クラスタ構成関連.
//!
//! なお、クラスタ構成の動的変更に関する詳細は、
//! [Raftの論文](https://raft.github.io/raft.pdf)の「6 Cluster membership changes」を参照のこと.
use std::cmp;
use std::collections::BTreeSet;

use crate::node::NodeId;

/// クラスタに属するメンバ群.
pub type ClusterMembers = BTreeSet<NodeId>;

/// クラスタの状態.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterState {
    /// 構成変更中ではなく安定している状態.
    Stable,

    /// 構成変更中で、新メンバ群のログを同期している状態.
    ///
    /// この状態では、リーダ選出やログのコミットにおいて、投票権を有するのは旧メンバのみである.
    CatchUp,

    /// 構成変更中で、新旧メンバ群の両方に合意が必要な状態.
    Joint,
}
impl ClusterState {
    /// 安定状態かどうかを判定する.
    pub fn is_stable(self) -> bool {
        self == ClusterState::Stable
    }

    /// 新旧混合状態かどうかを判定する.
    pub fn is_joint(self) -> bool {
        self == ClusterState::Joint
    }
}

/// クラスタ構成.
///
/// クラスタに属するメンバの集合に加えて、
/// 動的構成変更用の状態を管理する.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConfig {
    new: ClusterMembers,
    old: ClusterMembers,
    state: ClusterState,
}
impl ClusterConfig {
    /// 現在のクラスタ状態を返す.
    pub fn state(&self) -> ClusterState {
        self.state
    }

    /// 構成変更後のメンバ集合が返される.
    ///
    /// 安定状態では、現在のメンバ群が返される
    /// (i.e, `members`メソッドが返すメンバ群と等しい).
    pub fn new_members(&self) -> &ClusterMembers {
        &self.new
    }

    /// 構成変更前のメンバ集合が返される.
    ///
    /// 安定状態では、空集合が返される.
    pub fn old_members(&self) -> &ClusterMembers {
        &self.old
    }

    /// "プライマリメンバ"の全体集合Pを返す。
    ///
    /// プライマリメンバ: 合意に有効なメンバ
    /// サービスの整合性を担保するには、
    /// 単なる過半数以上の合意ではなく、「Pから過半数以上の合意」を得なくてはならない。
    ///
    /// メンバとプライマリメンバの区別は、構成変更過渡期で必要になる。
    /// 安定状態では、メンバはプライマリメンバとして見て良い。
    /// 構成変更時には、旧構成に属するメンバのみがプライマリメンバとなる。
    pub fn primary_members(&self) -> &ClusterMembers {
        match self.state {
            ClusterState::Stable => &self.new,
            ClusterState::CatchUp => &self.old,
            ClusterState::Joint => &self.old,
        }
    }

    /// クラスタに属するメンバ群を返す.
    ///
    /// 構成変更中の場合には、新旧両方のメンバの和集合が返される.
    pub fn members(&self) -> impl Iterator<Item = &NodeId> {
        self.new.union(&self.old)
    }

    /// このクラスタ構成に含まれるノードかどうかを判定する.
    pub fn is_known_node(&self, node: &NodeId) -> bool {
        self.new.contains(node) || self.old.contains(node)
    }

    /// 安定状態の`ClusterConfig`インスタンスを生成する.
    pub fn new(members: ClusterMembers) -> Self {
        ClusterConfig {
            new: members,
            old: ClusterMembers::default(),
            state: ClusterState::Stable,
        }
    }

    /// `state`を状態とする`ClusterConfig`インスタンスを生成する.
    pub fn with_state(
        new_members: ClusterMembers,
        old_members: ClusterMembers,
        state: ClusterState,
    ) -> Self {
        ClusterConfig {
            new: new_members,
            old: old_members,
            state,
        }
    }

    /// 構成を変更するために、
    /// `new`を（取り込みたい）新メンバ群とする
    /// `CatchUp`状態の`ClusterConfig`インスタンスを返す.
    pub(crate) fn start_config_change(&self, new: ClusterMembers) -> Self {
        ClusterConfig {
            new,
            old: self.primary_members().clone(),
            state: ClusterState::CatchUp,
        }
    }

    /// 次の状態に遷移する.
    ///
    /// # 状態遷移表
    ///
    ///                         v------|
    /// CatchUp --> Joint --> Stable --|
    pub(crate) fn to_next_state(&self) -> Self {
        match self.state {
            ClusterState::Stable => self.clone(),
            ClusterState::CatchUp => {
                let mut next = self.clone();
                next.state = ClusterState::Joint;
                next
            }
            ClusterState::Joint => {
                let mut next = self.clone();
                next.old = ClusterMembers::new(); // Stableではoldは空集合
                next.state = ClusterState::Stable;
                next
            }
        }
    }

    /// クラスタの合意値.
    //
    /// `f`は関数で、メンバごとの承認値を表す。
    /// f(m) = xは次の意味でm中で最大である:
    /// mの中でy < xであればyも承認済み。
    ///
    /// クラスタ合意値は「メンバの過半数によって承認されている最大の値」
    pub(crate) fn consensus_value<F, T>(&self, f: F) -> T
    where
        F: Fn(&NodeId) -> T,
        T: Ord + Copy + Default,
    {
        match self.state {
            ClusterState::Stable => median(&self.new, &f),
            ClusterState::CatchUp => median(&self.old, &f),
            ClusterState::Joint => {
                // joint consensus
                cmp::min(median(&self.new, &f), median(&self.old, &f))

                // FIX
                // median(self.new + self.old, f)でダメな理由は何？
            }
        }
    }

    /// Stable, Jointについては`consensus_value`メソッドと同様.
    ///
    /// Catchup(構成変更中)では、新旧メンバ群の両方から、
    /// 過半数の承認を要求するところが異なる.
    pub(crate) fn full_consensus_value<F, T>(&self, f: F) -> T
    where
        F: Fn(&NodeId) -> T,
        T: Ord + Copy + Default,
    {
        if self.state.is_stable() {
            median(&self.new, &f)
        } else {
            // joint & catchup consensus
            cmp::min(median(&self.new, &f), median(&self.old, &f))
        }
    }
}

// FIX: 「メンバの過半数によって承認されている最大の値」になっているか検証する
// 例: 4ノードで次のようになったとする
// v0 < v1 < v2 < v3
// v0は4ノードで保証
// v1は3ノードで保証
// ということになるので、過半数である3ノードでの保証はv1まで
// 従って ascending sort をした後に
// med-positonの値をとってやれば良い
//
// 現在の実装の過ちを指摘する場合は、具体的な状況とのsetで提供すること。
fn median<F, T>(members: &ClusterMembers, f: F) -> T
where
    F: Fn(&NodeId) -> T,
    T: Ord + Copy + Default,
{
    let mut values = members.iter().map(|n| f(n)).collect::<Vec<_>>();
    values.sort(); // v[0] < v[1] < ...
    values.reverse(); // v[0] > v[1] > ...
    if values.is_empty() {
        unreachable!("FIX");
    } else {
        // 2nの過半数はn+1 (0-originならn)
        // (2n)+1の過半数はn+1 (0-originならn)
        values[members.len() / 2]
    }
}
