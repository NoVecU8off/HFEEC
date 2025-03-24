// packet.rs
use std::sync::Arc;

use crossbeam::queue::ArrayQueue;

#[repr(C, align(64))]
pub struct PacketData {
    pub source_port: u16,
    pub dest_port: u16,
    pub queue_id: u16,
    pub source_ip: [u8; 16],
    pub source_ip_len: usize,
    pub dest_ip: [u8; 16],
    pub dest_ip_len: usize,
    pub data: *const u8,
    pub data_len: usize,
}

impl PacketData {
    pub fn new() -> Self {
        Self {
            source_port: 0,
            dest_port: 0,
            queue_id: 0,
            source_ip: [0; 16],
            source_ip_len: 0,
            dest_ip: [0; 16],
            dest_ip_len: 0,
            data: std::ptr::null(),
            data_len: 0,
        }
    }

    pub fn get_source_ip(&self) -> &[u8] {
        &self.source_ip[0..self.source_ip_len]
    }

    pub fn get_dest_ip(&self) -> &[u8] {
        &self.dest_ip[0..self.dest_ip_len]
    }

    pub fn get_data(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.data_len) }
    }
}

unsafe impl Send for PacketData {}

pub struct PacketDataPool {
    queue: Arc<ArrayQueue<Box<PacketData>>>,
}

impl PacketDataPool {
    pub fn new(capacity: usize) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));

        for _ in 0..capacity {
            let data = Box::new(PacketData::new());
            if queue.push(data).is_err() {
                panic!("Failed to push to packet pool queue");
            }
        }

        Self { queue }
    }

    pub fn acquire(&self) -> Box<PacketData> {
        match self.queue.pop() {
            Some(packet) => packet,
            None => Box::new(PacketData::new()),
        }
    }

    pub fn release(&self, mut packet: Box<PacketData>) {
        packet.source_ip_len = 0;
        packet.dest_ip_len = 0;
        packet.data = std::ptr::null();
        packet.data_len = 0;

        let _ = self.queue.push(packet);
    }
}
