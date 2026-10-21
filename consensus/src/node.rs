use crate::blocksync::service::BlockSyncService;
use crate::network::message::Message;
use crate::network::network::{Network, NetworkEvent, send_envelope};
use crate::protocol::driver::DriverInput;
use crate::protocol::driver::DriverInput::BlockSynced;
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::timeouts::TimeoutPolicy;
use crate::sm::{Effects, NodeError, NodeState};
use crate::storage::Storage;
use crate::timer::service::{TimerCommand, TimerEvent, TimerService};
use crate::types::block::Command;
use crate::types::clock::ClockSource;
use crate::types::crypto::Notary;
use crate::types::genesis::GenesisConfig;
use crate::types::mempool::Mempool;
use crate::types::output::Output;
use codec::DecodeAll;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Client<C> {
    tx: mpsc::UnboundedSender<C>,
}

impl<C> Client<C> {
    pub fn propose(&self, cmd: C) {
        let _ = self.tx.send(cmd);
    }
}


pub struct Node<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService<CS::Clock>,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy<CS::Clock>,
    CS: ClockSource,
    LP: LeaderPolicy,
    O: Output<S, C>,
> {
    network: N,
    blocksync: B,
    timers: T,
    state: NodeState<S, C, DB, M, TP, CS, LP>,
    out: O,
    cmd_ch: mpsc::UnboundedReceiver<C>,
}

impl<
    S: Scheme,
    C: Command,
    N: Network,
    B: BlockSyncService<S, C>,
    T: TimerService<CS::Clock>,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy<CS::Clock>,
    CS: ClockSource,
    LP: LeaderPolicy,
    O: Output<S, C>,
> Node<S, C, N, B, T, DB, M, TP, CS, LP, O>
{
    pub fn new(
        scheme: S,
        network: N,
        blocksync: B,
        timers: T,
        db: DB,
        mempool: M,
        timeouts: TP,
        source: CS,
        leaders: LP,
        out: O,
        genesis: GenesisConfig<S, C>,
        node_id: NodeID,
    ) -> Result<(Self, Client<C>), NodeError> {
        let notary = Notary::new(Arc::new(scheme));
        let state = NodeState::new(
            notary, db, mempool, node_id, source, timeouts, leaders, genesis,
        )?;
        let (cmd_tx, cmd_rc) = mpsc::unbounded_channel();

        Ok((
            Node {
                network,
                blocksync,
                timers,
                state,
                out,
                cmd_ch: cmd_rc,
            },
            Client { tx: cmd_tx },
        ))
    }

    pub async fn run(mut self) -> Result<(), NodeError> {
        loop {
            self.apply_state()?;

            tokio::select! {
                Some(cmd) = self.cmd_ch.recv() => {
                   self.state.add_command(cmd);
                }

                branch = self.blocksync.next_branch() => {
                    for block in branch {
                        self.state.add_event_to_queue(BlockSynced(block))
                    }
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

    fn apply_state(&mut self) -> Result<(), NodeError> {
        while let Some(effects) = self.state.apply()? {
            self.execute_effects(effects);
        }
        Ok(())
    }

    fn execute_effects(&mut self, effects: Effects<S, C, CS::Clock>) {
        for envelope in effects.net {
            let _ = send_envelope(envelope, &mut self.network);
        }
        for command in effects.timers {
            self.process_timer_command(command);
        }
        for missing in effects.sync {
            self.blocksync.request_block(missing);
        }
        for block in effects.committed {
            self.out.on_committed(block);
        }
    }

    fn process_timer_command(&mut self, command: TimerCommand<CS::Clock>) {
        match command {
            TimerCommand::SetEpochTimers(epoch) => self.timers.set_epoch_timers(epoch),
            TimerCommand::SetLockTimer(lock) => self.timers.set_lock_timer(lock),
            TimerCommand::ClearLockTimer(view) => self.timers.stop_lock_timer(view),
            TimerCommand::ClearViewTimersBelow(view) => self.timers.stop_view_timers_below(view),
        }
    }
}
