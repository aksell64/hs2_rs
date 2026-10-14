use crate::blocksync::service::BlockSyncService;
use crate::network::message::Message;
use crate::network::network::{Network, NetworkEvent, send_envelope};
use crate::sm::{Effects, NodeState};
use crate::storage::kv::{KVError, Storage};
use crate::timer::service::{TimerCommand, TimerEvent, TimerService};
use crate::types::block::Command;
use crate::types::mempool::Mempool;
use codec::DecodeAll;
use hotstuff_crypto::sign::Scheme;
use crate::protocol::driver::DriverEvent;

struct Node<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService,
    DB: Storage<S, C>,
    M: Mempool<C>,
> {
    network: N,
    blocksync: B,
    timers: T,
    state: NodeState<S, C, DB, M>,
}

impl<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService,
    DB: Storage<S, C>,
    M: Mempool<C>,
> Node<S, C, N, B, T, DB, M>
{
    pub async fn run(mut self) -> Result<(), KVError> {
        loop{
            while let Some(effects) = self.state.apply()? {
                self.execute_effects(effects);
            }

            tokio::select! {
                block = self.blocksync.next_block() => {
                    self.state.add_event_to_queue(DriverEvent::BlockSynced(block));
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
                        self.state.add_event_to_queue(DriverEvent::LockTimerFired(view));
                    },
                    TimerEvent::ViewTimeout(view) => {
                        self.state.add_event_to_queue(DriverEvent::ViewTimerFired(view));
                    }
                }
            }
        }
    }

    fn execute_effects(&mut self, effects: Effects<S, C>) {
        for envelope in effects.net {
            let _ = send_envelope(envelope, &mut self.network);
        }
        for command in effects.timers {
            match command {
                TimerCommand::StartView(view) => self.timers.start_view_timer(view),
                TimerCommand::StartLock(view) => self.timers.start_lock_timer(view),
                TimerCommand::StopLock(view) => self.timers.stop_lock_timer(view),
            }
        }
    }
}
