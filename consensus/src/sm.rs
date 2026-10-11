use crate::driver::driver::{Driver, DriverEvent, DriverOutput, DriverResult};
use crate::network::network::{send_envelope, Network};
use hotstuff_crypto::sign::Scheme;
use std::collections::VecDeque;
use crate::network::message::Envelope;
use crate::storage::kv::{KVError, Storage};
use crate::types::block::Command;

struct NodeState<S: Scheme, C: Command, DB: Storage<S, C>, N: Network> {
    driver: Driver<S, C>,
    queue: VecDeque<DriverEvent<S, C>>,
    db: DB,
    net: N,
}

impl<S: Scheme, C: Command, DB: Storage<S, C>, N: Network> NodeState<S, C, DB, N> {
    pub fn add_event_to_queue(&mut self, evt: DriverEvent<S, C>) {
        self.queue.push_back(evt);
    }

    fn apply_state(&mut self) -> Result<(), KVError> {
        let maybe_event = self.queue.pop_front();
        if let Some(event) = maybe_event {
            let driver_result = self.driver.apply(&mut self.db, event)?;
            self.process_net_messages(driver_result.net);
            self.process_driver_outs(driver_result.out)
        }
        Ok(())
    }


    fn process_net_messages(&mut self, envelopes: Vec<Envelope<S, C>>){
        for envelope in envelopes {
            let _ = send_envelope(envelope, &mut self.net);
        }
    }

    fn process_driver_outs(&mut self, outs: Vec<DriverOutput<S, C>>) {
        for out in outs {
            match out {
                DriverOutput::NewView(view) => {
                    self.queue.push_front(DriverEvent::EnterView(view));
                }
                DriverOutput::Replay(replay) => {
                    self.queue.push_front(replay);
                }
                DriverOutput::SetLockTimer(view) => {

                }
                DriverOutput::SetViewTimer(view) => {

                }
                DriverOutput::ClearLockTimer(view) => {

                }
            }
        }
    }
}
