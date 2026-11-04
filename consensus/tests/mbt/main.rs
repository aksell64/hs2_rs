mod spec;
mod world;

use quint_connect::*;
use world::World;

#[quint_run(
    spec = "../docs/spec/quint/models.qnt",
    main = "n4f0v20",
    step = "step_happy",
    max_samples = 5,
    max_steps = 200
)]
fn happy_path() -> impl Driver {
    World::new(4, [])
}
