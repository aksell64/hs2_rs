use crate::network::message::{Message, Recipient};
use crate::protocol::effects::{Effect, send_to, send_to_each};
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, ReadStorage, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::Command;
use crate::types::certificate::{BlockQC, TimeoutQC};
use crate::types::clock::{Clock, ClockSource};
use crate::types::crypto::Notary;
use crate::types::wish::Wish;
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

pub struct Pacemaker<S: Scheme, TP: TimeoutPolicy<CS::Clock>, CS: ClockSource, LP: LeaderPolicy> {
    notary: Notary<S>,
    id: NodeID,
    statuses: BTreeSet<NodeID>,
    sent_wish: BTreeSet<View>,
    relayed_tc: BTreeSet<View>,
    scheduled_epoch: Option<View>,
    lock_expired: View,
    source: CS,
    policy: TP,
    leaders: LP,
}

type PacemakerApply<S, C, CL> = Result<PacemakerResult<S, C, CL>, KVError>;

impl<S: Scheme, TP: TimeoutPolicy<CS::Clock>, CS: ClockSource, LP: LeaderPolicy>
    Pacemaker<S, TP, CS, LP>
{
    pub fn new(notary: Notary<S>, id: NodeID, source: CS, policy: TP, leaders: LP) -> Self {
        Self {
            notary,
            id,
            statuses: BTreeSet::from([id]),
            sent_wish: BTreeSet::new(),
            relayed_tc: BTreeSet::new(),
            scheduled_epoch: None,
            lock_expired: 0,
            source,
            policy,
            leaders,
        }
    }

    pub fn apply<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: PacemakerInput<S>,
    ) -> PacemakerApply<S, C, CS::Clock> {
        match event {
            PacemakerInput::ReceivedQC(qc) => self.process_qc(storage, qc),
            PacemakerInput::ViewTimerExpired(view) => self.on_timer_expired(storage, view),
            PacemakerInput::LockTimerExpired(view) => self.on_lock_timer_expired(storage, view),
            PacemakerInput::ReceivedTC(tc) => self.on_receive_tc(storage, tc),
            PacemakerInput::ReceivedStatus(status) => self.on_receive_status(storage, status),
        }
    }

    pub fn leader(&self, view: View) -> NodeID {
        self.leaders.leader(view)
    }

    pub fn is_leader(&self, view: View) -> bool {
        self.id == self.leader(view)
    }

    pub fn is_epoch_leader(&self, view: View) -> bool {
        self.leaders.epoch_leaders(view).contains(&self.id)
    }

    fn process_qc<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        qc: BlockQC<S>,
    ) -> PacemakerApply<S, C, CS::Clock> {
        if !self.notary.verify_qc(&qc) || qc.view() <= storage.get_high_qc()?.view() {
            return Ok(PacemakerResult::no_change());
        }
        storage.update_high_qc(qc.clone())?;
        if qc.view() >= storage.get_cur_view()? {
            let next_view = qc.view() + 1;
            let advanced = self.advance_view(storage, next_view)?;
            if self.leaders.is_epoch_start(next_view) {
                Ok(advanced.with_effects(vec![send_to(Recipient::All, Message::QCMsg(qc))]))
            } else {
                Ok(advanced)
            }
        } else {
            Ok(PacemakerResult::no_change())
        }
    }

    fn advance_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, CS::Clock> {
        let previous = storage.get_cur_view()?;
        if view <= previous {
            return Ok(PacemakerResult::no_change());
        }
        storage.update_cur_view(view)?;
        storage.update_cur_leader(self.leader(view))?;
        self.statuses = BTreeSet::from([self.id]);
        let start = self.source.now();
        let mut timers = vec![
            Effect::Timer(TimerCommand::ClearLockTimer(previous)),
            Effect::Timer(TimerCommand::ClearViewTimersBelow(view)),
        ];
        let epoch = self.leaders.epoch_of(view);
        if self.scheduled_epoch < Some(epoch) {
            self.scheduled_epoch = Some(epoch);
            let deadlines = self.policy.epoch_deadlines(
                self.leaders.epoch_start(view),
                view..=self.leaders.epoch_end(view),
                start,
            );
            timers.push(Effect::Timer(TimerCommand::SetEpochTimers(deadlines)));
        }
        let entered = self.enter_view(storage, view, start)?;
        Ok(entered.with_effects(timers))
    }

    fn enter_view<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
        start: CS::Clock,
    ) -> PacemakerApply<S, C, CS::Clock> {
        if view != storage.get_cur_view()? {
            return Ok(PacemakerResult::no_change());
        }
        let high_qc = storage.get_high_qc()?;
        let has_previous_qc = high_qc.view() + 1 == view;
        if self.is_leader(view) {
            if self.can_propose(storage, view)? {
                return Ok(Self::propose_now(view));
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

    pub fn can_propose<C: Command, DB: ReadStorage<S, C>>(
        &self,
        storage: &DB,
        view: View,
    ) -> Result<bool, KVError> {
        Ok(view == storage.get_cur_view()?
            && self.is_leader(view)
            && (storage.get_high_qc()?.view() + 1 == view || self.lock_expired == view))
    }

    fn propose_now<C: Command>(view: View) -> PacemakerResult<S, C, CS::Clock> {
        PacemakerResult {
            effects: Vec::new(),
            out: PacemakerOutput::ProposeNow(view),
        }
    }

    fn on_lock_timer_expired<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, CS::Clock> {
        if view != storage.get_cur_view()? || !self.is_leader(view) {
            return Ok(PacemakerResult::no_change());
        }
        self.lock_expired = view;
        Ok(Self::propose_now(view))
    }

    fn on_timer_expired<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> PacemakerApply<S, C, CS::Clock> {
        if view != storage.get_cur_view()? {
            return Ok(PacemakerResult::no_change());
        }
        let next = view + 1;
        if !self.leaders.is_epoch_start(next) {
            return self.advance_view(storage, next);
        }
        if !self.sent_wish.insert(next) {
            return Ok(PacemakerResult::no_change());
        }
        let Some(wish) = self.notary.sign(Wish { view: next }) else {
            return Ok(PacemakerResult::no_change());
        };

        let wishes = send_to_each(self.leaders.epoch_leaders(next), || {
            Message::WishMsg(wish.clone())
        });
        Ok(PacemakerResult::no_change().with_effects(wishes))
    }

    fn on_receive_tc<C: Command, DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        tc: TimeoutQC<S>,
    ) -> PacemakerApply<S, C, CS::Clock> {
        if !self.notary.verify_qc(&tc) || !self.relayed_tc.insert(tc.view()) {
            return Ok(PacemakerResult::no_change());
        }
        let leaders = self.leaders.epoch_leaders(tc.view());
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
    ) -> PacemakerApply<S, C, CS::Clock> {
        if !self.is_leader(status.view) {
            return Ok(PacemakerResult::no_change());
        }
        self.process_qc(storage, status.qc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::kv::MemoryKV;
    use crate::mocks::scheme::{TestCommand, TestScheme, TestSignature, digest, qc};
    use crate::mocks::timeouts::FixedPolicy;
    use crate::network::message::Envelope;
    use crate::protocol::leaders::RoundRobin;
    use crate::storage::WriteStorage;
    use crate::storage::kv_storage::KVStorage;
    use crate::storage::staged::StagedStorage;
    use crate::timer::queue::TimerQueue;
    use codec::Encode;
    use hotstuff_crypto::sign::PartialSignature;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct TestClock(u64);

    impl ClockSource for TestClock {
        type Clock = u64;

        fn now(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn qc_is_broadcast_only_when_entering_epoch_boundary() -> Result<(), KVError> {
        let id = NodeID::from(1);
        let scheme = Arc::new(TestScheme { n: 4, id: Some(id) });
        let notary = Notary::new(scheme.clone());
        let store = KVStorage::new(MemoryKV::new());
        let mut storage = StagedStorage::<TestScheme, TestCommand, _>::new(&store);
        storage.update_cur_view(0)?;
        storage.update_high_qc(qc(&scheme, 0, digest(0)))?;
        let tc = notary
            .aggregate(
                Wish { view: 8 },
                (0..3).map(|id| PartialSignature::new(NodeID::from(id), TestSignature)),
            )
            .unwrap();
        let mut invalid = qc(&scheme, 3, digest(3));
        invalid.subject.block = digest(99);
        let mut pacemaker = Pacemaker::new(
            notary,
            id,
            TestClock(0),
            FixedPolicy {
                view_timeout: 100,
                lock_timeout: 30,
            },
            RoundRobin::new(4),
        );
        let steps = [
            (
                "inside epoch",
                PacemakerInput::ReceivedQC(qc(&scheme, 2, digest(2))),
                3,
                2,
                0,
            ),
            ("invalid QC", PacemakerInput::ReceivedQC(invalid), 3, 2, 0),
            (
                "epoch boundary",
                PacemakerInput::ReceivedQC(qc(&scheme, 3, digest(3))),
                4,
                3,
                1,
            ),
            (
                "duplicate QC",
                PacemakerInput::ReceivedQC(qc(&scheme, 3, digest(3))),
                4,
                3,
                0,
            ),
            (
                "middle of epoch",
                PacemakerInput::ReceivedQC(qc(&scheme, 5, digest(5))),
                6,
                5,
                0,
            ),
            ("entry by TC", PacemakerInput::ReceivedTC(tc), 8, 5, 0),
            (
                "QC after TC",
                PacemakerInput::ReceivedQC(qc(&scheme, 7, digest(7))),
                8,
                7,
                0,
            ),
            (
                "later epoch",
                PacemakerInput::ReceivedQC(qc(&scheme, 11, digest(11))),
                12,
                11,
                1,
            ),
        ];

        for (case, input, view, high_qc_view, broadcasts) in steps {
            let result = pacemaker.apply(&mut storage, input)?;
            assert_eq!(storage.get_cur_view()?, view, "{case}");
            let high_qc = storage.get_high_qc()?;
            assert_eq!(high_qc.view(), high_qc_view, "{case}");
            let relayed: Vec<_> = result
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Send(Envelope {
                        to,
                        msg: Message::QCMsg(qc),
                    }) => Some((to, qc)),
                    _ => None,
                })
                .collect();
            assert_eq!(relayed.len(), broadcasts, "{case}");
            for (to, qc) in relayed {
                assert!(matches!(to, Recipient::All), "{case}");
                assert_eq!(qc.encode(), high_qc.encode(), "{case}");
            }
        }
        Ok(())
    }

    #[test]
    fn epoch_deadlines_survive_qc_jumps_and_timer_transitions() -> Result<(), KVError> {
        let id = NodeID::from(0);
        let scheme = Arc::new(TestScheme { n: 4, id: Some(id) });
        let store = KVStorage::new(MemoryKV::new());
        let mut storage = StagedStorage::<TestScheme, TestCommand, _>::new(&store);
        storage.update_cur_view(0)?;
        storage.update_high_qc(qc(&scheme, 0, digest(0)))?;
        let mut pacemaker = Pacemaker::new(
            Notary::new(scheme.clone()),
            id,
            TestClock(0),
            FixedPolicy {
                view_timeout: 100,
                lock_timeout: 30,
            },
            RoundRobin::new(4),
        );
        let mut timers = TimerQueue::new();
        let steps = [
            // Startup enters view 1 in the epoch starting at view 0.
            (
                0,
                PacemakerInput::ViewTimerExpired(0),
                1,
                vec![(1, 200), (2, 300), (3, 400)],
            ),
            // First entry into epoch 4..=7 skips views 4 and 5.
            (
                10,
                PacemakerInput::ReceivedQC(qc(&scheme, 5, digest(5))),
                6,
                vec![(6, 310), (7, 410)],
            ),
            // Advancing within the epoch preserves the original deadline.
            (
                20,
                PacemakerInput::ReceivedQC(qc(&scheme, 6, digest(6))),
                7,
                vec![(7, 410)],
            ),
            // The next epoch gets a fresh entry time.
            (
                30,
                PacemakerInput::ReceivedQC(qc(&scheme, 7, digest(7))),
                8,
                vec![(8, 130), (9, 230), (10, 330), (11, 430)],
            ),
            (
                130,
                PacemakerInput::ViewTimerExpired(8),
                9,
                vec![(9, 230), (10, 330), (11, 430)],
            ),
        ];

        for (now, input, view, expected) in steps {
            pacemaker.source.0 = now;
            let result = pacemaker.apply(&mut storage, input)?;
            for effect in result.effects {
                match effect {
                    Effect::Timer(TimerCommand::SetEpochTimers(epoch)) => {
                        timers.set_epoch_timers(epoch)
                    }
                    Effect::Timer(TimerCommand::SetLockTimer(lock)) => timers.set_lock_timer(lock),
                    Effect::Timer(TimerCommand::ClearLockTimer(view)) => {
                        timers.stop_lock_timer(view)
                    }
                    Effect::Timer(TimerCommand::ClearViewTimersBelow(view)) => {
                        timers.stop_view_timers_below(view)
                    }
                    Effect::Send(_) => {}
                }
            }
            assert_eq!(storage.get_cur_view()?, view);
            assert_eq!(
                timers.view_timers(),
                &BTreeMap::from_iter(expected),
                "view {view} at {now}"
            );
        }
        Ok(())
    }
}
