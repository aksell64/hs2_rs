use crate::network::message::{Message, Recipient};
use crate::protocol::effects::{Effect, send_to, send_to_each};
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::Command;
use crate::types::certificate::{BlockQC, TimeoutQC};
use crate::types::clock::{Clock, Source};
use crate::types::crypto::Notary;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::BTreeSet;

pub enum PacemakerInput<S: Scheme> {
    ReceivedQC(BlockQC<S>),
    ViewTimerExpired(View),
    LockTimerExpired(View),
    ReceivedTC(TimeoutQC<S>),
    ReceivedStatus(Status<S>),
}

pub struct Status<S: Scheme> {
    pub sender: NodeID,
    pub qc: BlockQC<S>,
    pub view: View,
}

pub enum PacemakerOutput {
    NoPacemakerOutput,
    ProposeNow(View),
}

pub struct PacemakerResult<S: Scheme, C: Command, CL: Clock> {
    pub effects: Vec<Effect<S, C, CL>>,
    pub out: PacemakerOutput,
}

impl<S: Scheme, C: Command, CL: Clock> PacemakerResult<S, C, CL> {
    fn no_change() -> Self {
        Self {
            effects: Vec::new(),
            out: PacemakerOutput::NoPacemakerOutput,
        }
    }

    fn with_effects(mut self, mut effects: Vec<Effect<S, C, CL>>) -> Self {
        effects.append(&mut self.effects);
        self.effects = effects;
        self
    }
}

pub struct Pacemaker<S: Scheme, TP: TimeoutPolicy, TS: Source<Clock = TP::Clock>> {
    notary: Notary<S>,
    id: NodeID,
    statuses: BTreeSet<NodeID>,
    sent_wish: BTreeSet<View>,
    relayed_tc: BTreeSet<View>,
    scheduled_epoch: Option<View>,
    source: TS,
    policy: TP,
}

type PacemakerApply<S, C, CL> = Result<PacemakerResult<S, C, CL>, KVError>;

impl<S: Scheme, TP: TimeoutPolicy, TS: Source<Clock = TP::Clock>> Pacemaker<S, TP, TS> {
    pub fn new(
        notary: Notary<S>,
        id: NodeID,
        source: TS,
        policy: TP,
    ) -> Self {
        Self {
            notary,
            id,
            statuses: BTreeSet::from([id]),
            sent_wish: BTreeSet::new(),
            relayed_tc: BTreeSet::new(),
            scheduled_epoch: None,
            source,
            policy,
        }
    }

    pub fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: PacemakerInput<S>,
    ) -> PacemakerApply<S, C, TP::Clock> {
        match event {
            PacemakerInput::ReceivedQC(qc) => self.process_qc(storage, qc),
            PacemakerInput::ViewTimerExpired(view) => self.on_timer_expired(storage, view),
            PacemakerInput::LockTimerExpired(view) => self.on_lock_timer_expired(storage, view),
            PacemakerInput::ReceivedTC(tc) => self.on_receive_tc(storage, tc),
            PacemakerInput::ReceivedStatus(status) => self.on_receive_status(storage, status),
        }
    }

    pub fn leader(&self, view: View) -> NodeID {
        let committee_size = View::from(self.notary.committee_size());
        NodeID::from((view % committee_size) as u32)
    }

    pub fn is_leader(&self, view: View) -> bool {
        self.id == self.leader(view)
    }

    pub fn is_epoch_leader(&self, view: View) -> bool {
        self.epoch_leaders(view).contains(&self.id)
    }

    fn epoch_length(&self) -> View {
        View::from((self.notary.committee_size().saturating_sub(1)) / 3 + 1)
    }

    fn epoch_of(&self, view: View) -> View {
        view / self.epoch_length()
    }

    fn epoch_end(&self, view: View) -> View {
        view - view % self.epoch_length() + self.epoch_length() - 1
    }

    fn epoch_leaders(&self, view: View) -> BTreeSet<NodeID> {
        let epoch_length = self.epoch_length();
        let first = view - view % epoch_length;
        (0..epoch_length).map(|k| self.leader(first + k)).collect()
    }

    fn process_qc<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        qc: BlockQC<S>,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if !self.notary.verify_qc(&qc) || qc.view() <= storage.get_high_qc()?.view() {
            return Ok(PacemakerResult::no_change());
        }
        storage.update_high_qc(qc.clone())?;
        if qc.view() >= storage.get_cur_view()? {
            self.advance_view(storage, qc.view() + 1)
        } else {
            Ok(PacemakerResult::no_change())
        }
    }

    fn advance_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, TP::Clock> {
        let previous = storage.get_cur_view()?;
        if view <= previous {
            return Ok(PacemakerResult::no_change());
        }
        storage.update_cur_view(view)?;
        storage.update_cur_leader(self.leader(view))?;
        storage.update_proposed(false)?;
        self.statuses = BTreeSet::from([self.id]);
        let start = self.source.now();
        let mut timers = vec![
            Effect::Timer(TimerCommand::ClearLockTimer(previous)),
            Effect::Timer(TimerCommand::ClearViewTimersBelow(view)),
        ];
        let epoch = self.epoch_of(view);
        if self.scheduled_epoch < Some(epoch) {
            self.scheduled_epoch = Some(epoch);
            let deadlines = self.policy.epoch_deadlines(view..=self.epoch_end(view), start);
            timers.push(Effect::Timer(TimerCommand::SetEpochTimers(deadlines)));
        }
        let entered = self.enter_view(storage, view, start)?;
        Ok(entered.with_effects(timers))
    }

    fn enter_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
        start: TP::Clock,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if view != storage.get_cur_view()? {
            return Ok(PacemakerResult::no_change());
        }
        let high_qc = storage.get_high_qc()?;
        let has_previous_qc = high_qc.view() + 1 == view;
        if self.is_leader(view) {
            if has_previous_qc {
                return Self::try_propose(storage, view);
            }
            let lock = self.policy.lock_deadline(view, start);
            let lock_timer = Effect::Timer(TimerCommand::SetLockTimer(lock));
            return Ok(PacemakerResult::no_change().with_effects(vec![lock_timer]));
        }
        if has_previous_qc {
            return Ok(PacemakerResult::no_change());
        }
        let status = send_to(
            Recipient::Node(self.leader(view)),
            Message::StatusMsg { qc: high_qc, view },
        );
        Ok(PacemakerResult::no_change().with_effects(vec![status]))
    }

    fn try_propose<C: Command, DB: ReadWriteStorage<S, C>>(
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if storage.get_proposed()? {
            return Ok(PacemakerResult::no_change());
        }
        storage.update_proposed(true)?;
        Ok(PacemakerResult {
            effects: Vec::new(),
            out: PacemakerOutput::ProposeNow(view),
        })
    }

    fn on_lock_timer_expired<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if view != storage.get_cur_view()? || !self.is_leader(view) {
            return Ok(PacemakerResult::no_change());
        }
        Self::try_propose(storage, view)
    }

    fn on_timer_expired<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if view != storage.get_cur_view()? {
            return Ok(PacemakerResult::no_change());
        }
        let next = view + 1;
        if !self.is_epoch_view(next) {
            return self.advance_view(storage, next);
        }
        if !self.sent_wish.insert(next) {
            return Ok(PacemakerResult::no_change());
        }
        let wishes = send_to_each(self.epoch_leaders(next), || Message::WishMsg(next));
        Ok(PacemakerResult::no_change().with_effects(wishes))
    }

    fn on_receive_tc<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        tc: TimeoutQC<S>,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if !self.notary.verify_qc(&tc) || !self.relayed_tc.insert(tc.view()) {
            return Ok(PacemakerResult::no_change());
        }
        let leaders = self.epoch_leaders(tc.view());
        let relay = if leaders.contains(&self.id) {
            vec![send_to(Recipient::All, Message::TCMsg(tc.clone()))]
        } else {
            send_to_each(leaders, || Message::TCMsg(tc.clone()))
        };
        let advanced = self.advance_view(storage, tc.view())?;
        Ok(advanced.with_effects(relay))
    }

    fn on_receive_status<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        status: Status<S>,
    ) -> PacemakerApply<S, C, TP::Clock> {
        if !self.is_leader(status.view) {
            return Ok(PacemakerResult::no_change());
        }
        self.process_qc(storage, status.qc)
    }

    fn is_epoch_view(&self, view: View) -> bool {
        view % self.epoch_length() == 0
    }
}
