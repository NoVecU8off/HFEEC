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
        // Обнуляем все указатели, но не освобождаем память, которая им принадлежит
        // MBuf уже должен быть освобожден вызывающей стороной
        packet.source_ip_ptr = std::ptr::null();
        packet.source_ip_len = 0;
        packet.dest_ip_ptr = std::ptr::null();
        packet.dest_ip_len = 0;
        packet.data_ptr = std::ptr::null();
        packet.data_len = 0;
        packet.mbuf_ptr = std::ptr::null_mut();

        let _ = self.queue.push(packet);
    }
}
