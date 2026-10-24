use std::time::Duration;

use tokio::time;

use crate::nockapp::driver::*;
use crate::nockapp::wire::Wire;
use crate::noun::slab::NounSlab;

pub enum TimerWire {
    Tick,
}

impl Wire for TimerWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "timer";
}

pub fn make_timer_driver(interval_secs: u64, timer_slab: NounSlab) -> IODriverFn {
    make_driver(move |handle| async move {
        let mut timer_interval = time::interval(Duration::from_secs(interval_secs));

        loop {
            tokio::select! {
                _ = timer_interval.tick() => {
                    let wire = TimerWire::Tick.to_wire();
                    handle.poke(wire, timer_slab.clone()).await?;
                }
            }
        }
    })
}
