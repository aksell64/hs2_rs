use std::collections::BTreeSet;
use std::sync::Arc;
use crate::types::View;
use crate::types::certificate::{CertifiedBlock, CertifiedView};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub enum PacemakerEvent<S: Scheme> {
    ProcessQC(CertifiedBlock<S>),
    HighQCApplied(CertifiedBlock<S>),
    TimerExpiredView(View),
    LockTimerExpired(View),
    EnterViewRequested(View),
    AdvanceViewRequested(View),
    LockTimerCanceled(View),
    ViewTimerStarted(View),
    ReceiveWish {
        sender: NodeID,
        view: View,
    },
    TimeoutQC(CertifiedView<S>),
    ReceiveStatus {
        sender: NodeID,
        qc: CertifiedBlock<S>,
        view: View,
    },
}

pub enum PacemakerOutput<S: Scheme> {
    HighQCUpdated(CertifiedBlock<S>),
    StartTimerView(View),
    StartLockTimer(View),
    CancelLockTimer {
        view: View,
        next: View,
    },
    ProposeNow(View),
    SendStatus {
        to: NodeID,
        qc: CertifiedBlock<S>,
        view: View,
    },
    AdvanceView(View),
    SendWish {
        to: BTreeSet<NodeID>,
        view: View,
    },
    EnterNewView(View),
    ReceiveWishIsLeader {
        sender: NodeID,
        view: View,
    },
    BroadcastTC(CertifiedView<S>),
    SendTCToLeader {
        to: BTreeSet<NodeID>,
        tc: CertifiedView<S>,
    },
    StatusIsLeader {
        sender: NodeID,
        qc: CertifiedBlock<S>,
        view: View,
    },
}


pub struct Pacemaker<S: Scheme> {
    scheme: Arc<S>,
    id: NodeID,
    high_qc: CertifiedBlock<S>,
    cur_view: View,
    cur_leader: NodeID,
    proposed: bool,
    statuses: BTreeSet<NodeID>,
    sent_wish: BTreeSet<View>,
    relayed_tc: BTreeSet<View>,
}

impl<S: Scheme> Pacemaker<S> {
    pub fn new(scheme: Arc<S>, id: NodeID, genesis_qc: CertifiedBlock<S>) -> Self {
        let mut pacemaker = Self {
            scheme,
            id,
            high_qc: genesis_qc,
            cur_view: 0,
            cur_leader: id,
            proposed: false,
            statuses: BTreeSet::from([id]),
            sent_wish: BTreeSet::new(),
            relayed_tc: BTreeSet::new(),
        };
        pacemaker.cur_leader = pacemaker.leader(0);
        pacemaker
    }

    pub fn apply(&mut self, event: PacemakerEvent<S>) -> Option<PacemakerOutput<S>> {
        match event {
            PacemakerEvent::ProcessQC(qc) => self.on_process_qc(qc),
            PacemakerEvent::HighQCApplied(qc) => self.on_high_qc_applied(qc),
            PacemakerEvent::TimerExpiredView(view) => self.on_timer_expired(view),
            PacemakerEvent::LockTimerExpired(view) => self.on_lock_timer_expired(view),
            PacemakerEvent::EnterViewRequested(view) => self.enter_view(view),
            PacemakerEvent::AdvanceViewRequested(view) => self.on_advance_view(view),
            PacemakerEvent::LockTimerCanceled(view) => self.on_lock_timer_canceled(view),
            PacemakerEvent::ViewTimerStarted(view) => self.on_view_timer_started(view),
            PacemakerEvent::ReceiveWish { sender, view } => self.on_receive_wish(sender, view),
            PacemakerEvent::TimeoutQC(tc) => self.on_receive_tc(tc),
            PacemakerEvent::ReceiveStatus { sender, qc, view } => self.on_receive_status(sender, qc, view),
        }
    }

    pub fn leader(&self, view: View) -> NodeID {
        let committee_size = View::from(self.scheme.committee_size());
        NodeID::from((view % committee_size) as u32)
    }

    pub fn is_leader(&self, view: View) -> bool {
        self.id == self.leader(view)
    }

    pub fn get_current_view(&self) -> View {
        self.cur_view
    }

    pub fn get_high_qc(&self) -> &CertifiedBlock<S> {
        &self.high_qc
    }

    fn epoch_length(&self) -> View {
        View::from((self.scheme.committee_size().saturating_sub(1)) / 3 + 1)
    }

    fn epoch_leaders(&self, view: View) -> BTreeSet<NodeID> {
        let epoch_length = self.epoch_length();
        let first = view - view % epoch_length;
        (0..epoch_length).map(|k| self.leader(first + k)).collect()
    }

    fn on_process_qc(&mut self, qc: CertifiedBlock<S>) -> Option<PacemakerOutput<S>> {
        if !self.scheme.verify_certificate(&qc.certificate) || qc.view <= self.high_qc.view {
            return None;
        }
        self.high_qc = qc.clone();
        Some(PacemakerOutput::HighQCUpdated(qc))
    }

    fn on_high_qc_applied(&mut self, qc: CertifiedBlock<S>) -> Option<PacemakerOutput<S>> {
        (qc.view >= self.cur_view).then(|| PacemakerOutput::AdvanceView(qc.view + 1))
    }

    fn on_timer_expired(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        if view != self.cur_view {
            return None;
        }
        let next = view + 1;
        if next % self.epoch_length() != 0 {
            return Some(PacemakerOutput::AdvanceView(next));
        }
        if !self.sent_wish.insert(next) {
            return None;
        }
        Some(PacemakerOutput::SendWish { to: self.epoch_leaders(next), view: next })
    }

    fn on_lock_timer_expired(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        if view != self.cur_view || !self.is_leader(view) {
            return None;
        }
        self.try_propose()
    }

    fn enter_view(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        if view != self.cur_view {
            return None;
        }
        let has_previous_qc = self.high_qc.view + 1 == view;
        if self.is_leader(view) {
            if has_previous_qc {
                self.try_propose()
            } else {
                Some(PacemakerOutput::StartLockTimer(view))
            }
        } else if !has_previous_qc {
            Some(PacemakerOutput::SendStatus {
                to: self.leader(view),
                qc: self.high_qc.clone(),
                view,
            })
        } else {
            None
        }
    }

    fn try_propose(&mut self) -> Option<PacemakerOutput<S>> {
        if self.proposed {
            return None;
        }
        self.proposed = true;
        Some(PacemakerOutput::ProposeNow(self.cur_view))
    }

    fn on_advance_view(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        if view <= self.cur_view {
            return None;
        }
        let previous = self.cur_view;
        self.cur_view = view;
        self.cur_leader = self.leader(view);
        self.proposed = false;
        self.statuses = BTreeSet::from([self.id]);
        Some(PacemakerOutput::CancelLockTimer { view: previous, next: view })
    }

    fn on_lock_timer_canceled(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        (view == self.cur_view).then_some(PacemakerOutput::StartTimerView(view))
    }

    fn on_view_timer_started(&mut self, view: View) -> Option<PacemakerOutput<S>> {
        (view == self.cur_view).then_some(PacemakerOutput::EnterNewView(view))
    }

    fn on_receive_wish(&mut self, sender: NodeID, view: View) -> Option<PacemakerOutput<S>> {
        self.epoch_leaders(view)
            .contains(&self.id)
            .then_some(PacemakerOutput::ReceiveWishIsLeader { sender, view })
    }

    fn on_receive_tc(&mut self, tc: CertifiedView<S>) -> Option<PacemakerOutput<S>> {
        if !self.scheme.verify_certificate(&tc.certificate) || !self.relayed_tc.insert(tc.view) {
            return None;
        }
        let leaders = self.epoch_leaders(tc.view);
        if leaders.contains(&self.id) {
            Some(PacemakerOutput::BroadcastTC(tc))
        } else {
            Some(PacemakerOutput::SendTCToLeader { to: leaders, tc })
        }
    }

    fn on_receive_status(&mut self, sender: NodeID, qc: CertifiedBlock<S>, view: View) -> Option<PacemakerOutput<S>> {
        self.is_leader(view)
            .then_some(PacemakerOutput::StatusIsLeader { sender, qc, view })
    }
}
