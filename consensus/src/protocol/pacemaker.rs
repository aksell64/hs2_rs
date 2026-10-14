use std::collections::BTreeSet;
use std::sync::Arc;
use crate::storage::kv::{KVError, ReadStorage, ReadWriteStorage};
use crate::types::View;
use crate::types::block::Command;
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
    statuses: BTreeSet<NodeID>,
    sent_wish: BTreeSet<View>,
    relayed_tc: BTreeSet<View>,
}

type PacemakerResult<S> = Result<Option<PacemakerOutput<S>>, KVError>;

impl<S: Scheme> Pacemaker<S> {
    pub fn new(scheme: Arc<S>, id: NodeID) -> Self {
        Self {
            scheme,
            id,
            statuses: BTreeSet::from([id]),
            sent_wish: BTreeSet::new(),
            relayed_tc: BTreeSet::new(),
        }
    }

    pub fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: PacemakerEvent<S>,
    ) -> PacemakerResult<S> {
        match event {
            PacemakerEvent::ProcessQC(qc) => self.on_process_qc(storage, qc),
            PacemakerEvent::HighQCApplied(qc) => self.on_high_qc_applied(storage, qc),
            PacemakerEvent::TimerExpiredView(view) => self.on_timer_expired(storage, view),
            PacemakerEvent::LockTimerExpired(view) => self.on_lock_timer_expired(storage, view),
            PacemakerEvent::EnterViewRequested(view) => self.enter_view(storage, view),
            PacemakerEvent::AdvanceViewRequested(view) => self.on_advance_view(storage, view),
            PacemakerEvent::LockTimerCanceled(view) => self.on_lock_timer_canceled(storage, view),
            PacemakerEvent::ViewTimerStarted(view) => self.on_view_timer_started(storage, view),
            PacemakerEvent::ReceiveWish { sender, view } => Ok(self.on_receive_wish(sender, view)),
            PacemakerEvent::TimeoutQC(tc) => Ok(self.on_receive_tc(tc)),
            PacemakerEvent::ReceiveStatus { sender, qc, view } => {
                Ok(self.on_receive_status(sender, qc, view))
            }
        }
    }

    pub fn leader(&self, view: View) -> NodeID {
        let committee_size = View::from(self.scheme.committee_size());
        NodeID::from((view % committee_size) as u32)
    }

    pub fn is_leader(&self, view: View) -> bool {
        self.id == self.leader(view)
    }

    fn epoch_length(&self) -> View {
        View::from((self.scheme.committee_size().saturating_sub(1)) / 3 + 1)
    }

    fn epoch_leaders(&self, view: View) -> BTreeSet<NodeID> {
        let epoch_length = self.epoch_length();
        let first = view - view % epoch_length;
        (0..epoch_length).map(|k| self.leader(first + k)).collect()
    }

    fn on_process_qc<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        qc: CertifiedBlock<S>,
    ) -> PacemakerResult<S> {
        if !self.scheme.verify_certificate(&qc.certificate)
            || qc.view <= storage.get_high_qc()?.view
        {
            return Ok(None);
        }
        storage.update_high_qc(qc.clone())?;
        Ok(Some(PacemakerOutput::HighQCUpdated(qc)))
    }

    fn on_high_qc_applied<C: Command, DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        qc: CertifiedBlock<S>,
    ) -> PacemakerResult<S> {
        let cur_view = storage.get_cur_view()?;
        Ok((qc.view >= cur_view).then(|| PacemakerOutput::AdvanceView(qc.view + 1)))
    }

    fn on_timer_expired<C: Command, DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        view: View,
    ) -> PacemakerResult<S> {
        if view != storage.get_cur_view()? {
            return Ok(None);
        }
        let next = view + 1;
        if next % self.epoch_length() != 0 {
            return Ok(Some(PacemakerOutput::AdvanceView(next)));
        }
        if !self.sent_wish.insert(next) {
            return Ok(None);
        }
        Ok(Some(PacemakerOutput::SendWish {
            to: self.epoch_leaders(next),
            view: next,
        }))
    }

    fn on_lock_timer_expired<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerResult<S> {
        if view != storage.get_cur_view()? || !self.is_leader(view) {
            return Ok(None);
        }
        Self::try_propose(storage, view)
    }

    fn enter_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerResult<S> {
        if view != storage.get_cur_view()? {
            return Ok(None);
        }
        let high_qc = storage.get_high_qc()?;
        let has_previous_qc = high_qc.view + 1 == view;
        if self.is_leader(view) {
            if has_previous_qc {
                Self::try_propose(storage, view)
            } else {
                Ok(Some(PacemakerOutput::StartLockTimer(view)))
            }
        } else if !has_previous_qc {
            Ok(Some(PacemakerOutput::SendStatus {
                to: self.leader(view),
                qc: high_qc,
                view,
            }))
        } else {
            Ok(None)
        }
    }

    fn try_propose<C: Command, DB: ReadWriteStorage<S, C>>(
        storage: &mut DB,
        view: View,
    ) -> PacemakerResult<S> {
        if storage.get_proposed()? {
            return Ok(None);
        }
        storage.update_proposed(true)?;
        Ok(Some(PacemakerOutput::ProposeNow(view)))
    }

    fn on_advance_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerResult<S> {
        let previous = storage.get_cur_view()?;
        if view <= previous {
            return Ok(None);
        }
        storage.update_cur_view(view)?;
        storage.update_cur_leader(self.leader(view))?;
        storage.update_proposed(false)?;
        self.statuses = BTreeSet::from([self.id]);
        Ok(Some(PacemakerOutput::CancelLockTimer {
            view: previous,
            next: view,
        }))
    }

    fn on_lock_timer_canceled<C: Command, DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        view: View,
    ) -> PacemakerResult<S> {
        Ok((view == storage.get_cur_view()?).then_some(PacemakerOutput::StartTimerView(view)))
    }

    fn on_view_timer_started<C: Command, DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        view: View,
    ) -> PacemakerResult<S> {
        Ok((view == storage.get_cur_view()?).then_some(PacemakerOutput::EnterNewView(view)))
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

    fn on_receive_status(
        &mut self,
        sender: NodeID,
        qc: CertifiedBlock<S>,
        view: View,
    ) -> Option<PacemakerOutput<S>> {
        self.is_leader(view)
            .then_some(PacemakerOutput::StatusIsLeader { sender, qc, view })
    }
}
