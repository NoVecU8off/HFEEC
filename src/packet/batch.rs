// src/packet/batch.rs
use std::mem::MaybeUninit;
use std::sync::Arc;

use crossbeam::queue::ArrayQueue;

use crate::dpdk::ffi::RteMbuf;
use crate::dpdk::wrappers::SendableMbufBuffer;
use crate::packet::data::PacketData;
use crate::packet::pool::PacketDataPool;

/// Структура для пакетной обработки без копирования данных
#[repr(C, align(64))]
pub struct PacketBatch {
    /// Размер batch в пакетах
    capacity: usize,
    /// Текущее количество пакетов в batch
    size: usize,
    /// Ссылка на пул пакетов
    packet_pool: Arc<PacketDataPool>,
    /// Буфер указателей на заполненные структуры PacketData
    /// (использует MaybeUninit для предотвращения инициализации ненужных элементов)
    packets: Box<[MaybeUninit<PacketData>]>,
    /// Буфер указателей на пакеты DPDK для пакетной отправки
    mbufs: SendableMbufBuffer,
}

impl PacketBatch {
    /// Создает новый batch пакетов с указанной емкостью
    pub fn new(capacity: usize, packet_pool: Arc<PacketDataPool>) -> Self {
        let packets = (0..capacity)
            .map(|_| MaybeUninit::uninit())
            .collect::<Vec<_>>()
            .into_boxed_slice();

        let mbufs = SendableMbufBuffer::new(capacity);

        Self {
            capacity,
            size: 0,
            packet_pool,
            packets,
            mbufs,
        }
    }

    /// Получает указатель на буфер DPDK mbufs для использования в rte_eth_rx_burst
    pub fn get_mbufs_ptr(&mut self) -> *mut *mut RteMbuf {
        self.mbufs.as_mut_ptr()
    }

    /// Заполняет batch данными после вызова rte_eth_rx_burst
    pub fn fill_from_rx_burst(&mut self, nb_rx: usize, queue_id: u16) {
        self.size = nb_rx;

        for i in 0..nb_rx {
            let mut packet = self.packet_pool.acquire();

            let mbuf = self.mbufs.get(i);

            packet.queue_id = queue_id;
            packet.mbuf_ptr = mbuf;

            let mut src_ip_ptr = std::ptr::null_mut();
            let mut src_ip_len: u32 = 0;
            let mut dst_ip_ptr = std::ptr::null_mut();
            let mut dst_ip_len: u32 = 0;
            let mut src_port: u16 = 0;
            let mut dst_port: u16 = 0;
            let mut data_ptr = std::ptr::null_mut();
            let mut data_len: u32 = 0;

            let ret = unsafe {
                crate::dpdk::ffi::dpdk_extract_packet_data(
                    mbuf,
                    &mut src_ip_ptr,
                    &mut src_ip_len,
                    &mut dst_ip_ptr,
                    &mut dst_ip_len,
                    &mut src_port,
                    &mut dst_port,
                    &mut data_ptr,
                    &mut data_len,
                )
            };

            if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                packet.source_port = src_port;
                packet.dest_port = dst_port;
                packet.source_ip_ptr = src_ip_ptr;
                packet.source_ip_len = src_ip_len as usize;
                packet.dest_ip_ptr = dst_ip_ptr;
                packet.dest_ip_len = dst_ip_len as usize;
                packet.data_ptr = data_ptr;
                packet.data_len = data_len as usize;

                self.packets[i].write(packet);
            } else {
                unsafe { crate::dpdk::ffi::rte_pktmbuf_free(mbuf) };
                self.size -= 1;

                self.packet_pool.release(packet);
            }
        }
    }

    /// Обрабатывает все пакеты в batch с использованием заданного обработчика
    pub fn process_all<F>(&self, handler: F)
    where
        F: Fn(u16, &PacketData),
    {
        for i in 0..self.size {
            let packet = unsafe { &*self.packets[i].as_ptr() };

            handler(packet.queue_id, packet);
        }
    }

    /// Освобождает все ресурсы batch и возвращает пакеты в пул
    pub fn release(&mut self) {
        for i in 0..self.size {
            let mbuf = self.mbufs.get(i);
            if !mbuf.is_null() {
                unsafe { crate::dpdk::ffi::rte_pktmbuf_free(mbuf) };
                self.mbufs.set(i, std::ptr::null_mut());
            }

            let packet = unsafe { self.packets[i].assume_init_read() };
            self.packet_pool.release(packet);
        }

        self.size = 0;
    }

    /// Получает текущий размер batch
    pub fn size(&self) -> usize {
        self.size
    }

    /// Проверяет, пуст ли batch
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Возвращает ссылку на i-й пакет
    pub fn get(&self, i: usize) -> Option<&PacketData> {
        if i < self.size {
            Some(unsafe { &*self.packets[i].as_ptr() })
        } else {
            None
        }
    }
}

impl Drop for PacketBatch {
    fn drop(&mut self) {
        self.release();
    }
}

/// Пул предварительно выделенных структур PacketBatch
pub struct PacketBatchPool {
    /// Очередь с доступными batch структурами
    queue: Arc<ArrayQueue<PacketBatch>>,
    /// Размер каждого batch
    batch_size: usize,
    /// Пул пакетов, используемый для создания новых batch
    packet_pool: Arc<PacketDataPool>,
}

impl PacketBatchPool {
    /// Создает новый пул batch структур
    pub fn new(num_batches: usize, batch_size: usize, packet_pool: Arc<PacketDataPool>) -> Self {
        let queue = Arc::new(ArrayQueue::new(num_batches));

        for _ in 0..num_batches {
            let batch = PacketBatch::new(batch_size, Arc::clone(&packet_pool));
            let _ = queue.push(batch);
        }

        Self {
            queue,
            batch_size,
            packet_pool,
        }
    }

    /// Получает batch из пула
    pub fn acquire(&self) -> PacketBatch {
        match self.queue.pop() {
            Some(batch) => batch,
            None => {
                println!("Warning: Batch pool is empty, creating new batch");
                PacketBatch::new(self.batch_size, Arc::clone(&self.packet_pool))
            }
        }
    }

    /// Возвращает batch в пул
    pub fn release(&self, mut batch: PacketBatch) {
        batch.release();

        if self.queue.push(batch).is_err() {
            println!("Warning: Failed to return batch to pool (pool is full)");
        }
    }
}
