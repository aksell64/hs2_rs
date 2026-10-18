use crate::blocksync::service::BlockSyncService;
use crate::network::message::Message;
use crate::network::network::{Network, NetworkEvent, send_envelope};
use crate::protocol::driver::DriverInput;
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::timeouts::TimeoutPolicy;
use crate::sm::{Effects, NodeState};
use crate::storage::{KVError, Storage};
use crate::timer::service::{TimerCommand, TimerEvent, TimerService};
use crate::types::block::Command;
use crate::types::clock::Source;
use crate::types::mempool::Mempool;
use codec::DecodeAll;
use hotstuff_crypto::sign::Scheme;

struct Node<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService<TP::Clock>,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy,
    TS: Source<Clock = TP::Clock>,
    LP: LeaderPolicy,
> {
    network: N,
    blocksync: B,
    timers: T,
    state: NodeState<S, C, DB, M, TP, TS, LP>,
}

impl<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService<TP::Clock>,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy,
    TS: Source<Clock = TP::Clock>,
    LP: LeaderPolicy,
> Node<S, C, N, B, T, DB, M, TP, TS, LP>
{
    pub async fn run(mut self) -> Result<(), KVError> {
        loop {
            while let Some(effects) = self.state.apply()? {
                self.execute_effects(effects);
            }

            tokio::select! {
                block = self.blocksync.next_block() => {
                    self.state.add_event_to_queue(DriverInput::BlockSynced(block));
                },

                Some(event) = self.network.next() => match event {
                    NetworkEvent::Message { from, bytes } => {
                        if let Ok(msg) = Message::<S, C>::decode_all(&mut &bytes[..]) {
                            self.state.add_event_to_queue(msg.into_event(from));
                        }
                    }
                },

                event = self.timers.next_event() => match event {
                    TimerEvent::LockTimeout(view) => {
                        self.state.add_event_to_queue(DriverInput::LockTimerFired(view));
                    },
                    TimerEvent::ViewTimeout(view) => {
                        self.state.add_event_to_queue(DriverInput::ViewTimerFired(view));
                    }
                }
            }
        }
    }

    fn execute_effects(&mut self, effects: Effects<S, C, TP::Clock>) {
        for envelope in effects.net {
            let _ = send_envelope(envelope, &mut self.network);
        }
        for command in effects.timers {
            self.process_timer_command(command);
        }
    }

    fn process_timer_command(&mut self, command: TimerCommand<TP::Clock>) {
        match command {
            TimerCommand::SetEpochTimers(epoch) => self.timers.set_epoch_timers(epoch),
            TimerCommand::SetLockTimer(lock) => self.timers.set_lock_timer(lock),
            TimerCommand::ClearLockTimer(view) => self.timers.stop_lock_timer(view),
            TimerCommand::ClearViewTimersBelow(view) => self.timers.stop_view_timers_below(view),
        }
    }
}

