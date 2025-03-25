//pool.rs
use std::sync::Arc;

use crossbeam::queue::ArrayQueue;

use super::packet::PacketData;

pub struct PacketDataPool {
    queue: Arc<ArrayQueue<PacketData>>,
}

impl PacketDataPool {
    pub fn new(capacity: usize) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));

        for _ in 0..capacity {
            let data = PacketData::new();
            if queue.push(data).is_err() {
                panic!("Failed to push to packet pool queue");
            }
        }

        Self { queue }
    }

    pub fn acquire(&self) -> PacketData {
        match self.queue.pop() {
            Some(packet) => packet,
            None => PacketData::new(),
        }
    }

    pub fn release(&self, mut packet: PacketData) {
        packet.source_ip_len = 0;
        packet.dest_ip_len = 0;
        packet.data = std::ptr::null();
        packet.data_len = 0;

        let _ = self.queue.push(packet);
    }
}
