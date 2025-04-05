use crossbeam::queue::ArrayQueue;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::numa::ffi::NumaAllocator;
use crate::packet::data::PacketData;

/// Пул пакетов данных с поддержкой NUMA
pub struct PacketDataPool {
    /// Очередь с пакетами
    queue: Arc<ArrayQueue<PacketData>>,
    /// NUMA-узел, на котором выделена память
    numa_node: Option<usize>,
    /// Информация о выделенной памяти для корректного освобождения
    allocated_memory: Option<(*mut c_void, usize)>,
}

impl PacketDataPool {
    /// Создает новый пул пакетов, оптимально в памяти конкретного узла NUMA
    pub fn new(capacity: usize, numa_node: Option<usize>) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));
        let mut allocated_memory = None;

        if let Some(node) = numa_node {
            if NumaAllocator::is_available() {
                println!(
                    "Creating packet pool with NUMA-optimized memory on node {}",
                    node
                );

                let packet_size = std::mem::size_of::<PacketData>();
                let aligned_size = (packet_size + 63) & !63;
                let total_size = aligned_size * capacity;

                let memory = NumaAllocator::alloc_on_node(total_size, node);

                if !memory.is_null() {
                    allocated_memory = Some((memory, total_size));

                    let memory_slice =
                        unsafe { std::slice::from_raw_parts_mut(memory as *mut u8, total_size) };

                    for i in 0..capacity {
                        let offset = i * packet_size;
                        let packet_ptr = memory_slice[offset..].as_mut_ptr() as *mut PacketData;

                        unsafe {
                            std::ptr::write(packet_ptr, PacketData::new());

                            if queue.push(std::ptr::read(packet_ptr)).is_err() {
                                break;
                            }
                        }
                    }

                    println!("Successfully allocated NUMA-optimized memory for packet pool");
                } else {
                    println!("Warning: Failed to allocate NUMA memory, falling back to regular allocation");
                }
            }
        }

        if allocated_memory.is_none() {
            println!("Creating packet pool with regular memory allocation");
            for _ in 0..capacity {
                let data = PacketData::new();
                let _ = queue.push(data);
            }
        }

        Self {
            queue,
            numa_node,
            allocated_memory,
        }
    }

    /// Получает пакет из пула
    pub fn acquire(&self) -> PacketData {
        match self.queue.pop() {
            Some(packet) => packet,
            None => {
                println!("Warning: Packet pool is empty, creating new packet");
                PacketData::new()
            }
        }
    }

    /// Возвращает пакет в пул
    pub fn release(&self, mut packet: PacketData) {
        packet.reset();

        if self.queue.push(packet).is_err() {
            println!("Warning: Failed to return packet to pool (pool is full)");
        }
    }

    /// Возвращает NUMA-узел, на котором выделена память
    pub fn get_numa_node(&self) -> Option<usize> {
        self.numa_node
    }
}

impl Drop for PacketDataPool {
    fn drop(&mut self) {
        if let Some((ptr, size)) = self.allocated_memory {
            println!("Freeing NUMA-allocated memory for packet pool");
            NumaAllocator::free(ptr, size);
        }
    }
}

unsafe impl Send for PacketDataPool {}

unsafe impl Sync for PacketDataPool {}
