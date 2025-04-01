// src/numa/node.rs
use core_affinity::CoreId;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

use crate::cpu::topology::CpuTopology;
use crate::dpdk::config::DpdkConfig;
use crate::numa::ffi::NumaAllocator;
use crate::numa::topology::NumaTopology;
use crate::packet::data::PacketData;
use crate::packet::pool::PacketDataPool;

/// Информация о DPDK порте
#[derive(Debug)]
pub struct DpdkPort {
    pub port_id: u16,
    pub if_name: String,
    pub num_rx_queues: u16,
    pub num_tx_queues: u16,
}

/// Рабочий поток
#[derive(Debug)]
pub struct Worker {
    pub thread: Option<JoinHandle<()>>,
    pub core_id: CoreId,
    pub port_id: u16,
    pub queue_id: u16,
}

/// Тип обработчика пакетов
pub type PacketHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

/// Автономный узел NUMA
pub struct NumaNode {
    /// ID узла NUMA
    pub node_id: usize,
    /// Список локальных CPU
    pub local_cpus: Vec<CoreId>,
    /// Список локальных NIC (сетевых карт)
    pub local_ports: Vec<DpdkPort>,
    /// Пул пакетов, выделенный в локальной памяти
    pub packet_pool: Option<Arc<PacketDataPool>>,
    /// Рабочие потоки
    pub workers: Vec<Worker>,
    /// Флаг работы
    pub running: Arc<AtomicBool>,
}

impl NumaNode {
    /// Создает новый узел NUMA
    pub fn new(node_id: usize, cpu_topology: &CpuTopology, _numa_topology: &NumaTopology) -> Self {
        // Получаем список CPU для этого узла
        let local_cpus = if NumaAllocator::is_available() {
            let numa_cpus = NumaAllocator::get_node_cpus(node_id);

            numa_cpus
                .into_iter()
                .filter(|&id| id != 0) // Исключаем ядро 0
                .filter(|&id| cpu_topology.is_primary_logical_core(id)) // Берем только первые логические ядра (без HT)
                .map(|id| CoreId { id })
                .collect()
        } else {
            // Если NUMA не доступна, берем все ядра из CPU топологии
            cpu_topology.get_filtered_core_ids()
        };

        println!(
            "Created NUMA node {} with {} CPU cores",
            node_id,
            local_cpus.len()
        );

        NumaNode {
            node_id,
            local_cpus,
            local_ports: Vec::new(),
            packet_pool: None,
            workers: Vec::new(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Проверяет, принадлежит ли сетевая карта этому узлу NUMA
    pub fn is_local_nic(&self, if_name: &str, numa_topology: &NumaTopology) -> bool {
        if let Some(nic_node) = numa_topology.get_nic_node(if_name) {
            nic_node == self.node_id
        } else {
            // Если не можем определить узел для NIC, считаем что да
            // (это упрощает логику для систем без NUMA)
            true
        }
    }

    /// Регистрирует локальную сетевую карту
    pub fn register_port(
        &mut self,
        port_id: u16,
        if_name: &str,
        num_rx_queues: u16,
        num_tx_queues: u16,
        numa_topology: &NumaTopology,
    ) -> bool {
        // Проверяем, принадлежит ли NIC этому узлу
        if !self.is_local_nic(if_name, numa_topology) {
            return false;
        }

        println!(
            "Registering port {} ({}) on NUMA node {}",
            port_id, if_name, self.node_id
        );

        self.local_ports.push(DpdkPort {
            port_id,
            if_name: if_name.to_string(),
            num_rx_queues,
            num_tx_queues,
        });

        true
    }

    /// Инициализирует пул пакетов в локальной памяти
    pub fn init_packet_pool(&mut self, capacity: usize) -> Result<(), String> {
        // Создаем пул пакетов, привязанный к этому узлу NUMA
        let pool = PacketDataPool::new(capacity, Some(self.node_id));
        self.packet_pool = Some(Arc::new(pool));

        println!(
            "Initialized packet pool with capacity {} on NUMA node {}",
            capacity, self.node_id
        );
        Ok(())
    }

    /// Запускает рабочие потоки для обработки пакетов
    pub fn start_workers(
        &mut self,
        packet_handler: PacketHandler,
        burst_size: u32,
    ) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("Workers already running".to_string());
        }

        // Проверяем, что у нас есть пул пакетов
        let packet_pool = match &self.packet_pool {
            Some(pool) => Arc::clone(pool),
            None => {
                // Если пул не был создан, создаем его сейчас
                self.init_packet_pool((burst_size * 4) as usize)?;
                Arc::clone(self.packet_pool.as_ref().unwrap())
            }
        };

        self.running.store(true, Ordering::SeqCst);

        // Для каждого порта на этом узле
        for port in &self.local_ports {
            let port_id = port.port_id;
            let num_rx_queues = port.num_rx_queues;

            println!(
                "Starting {} worker threads for port {} on NUMA node {}",
                num_rx_queues, port_id, self.node_id
            );

            // Проверяем, что у нас есть достаточно CPU
            if self.local_cpus.is_empty() {
                return Err(format!("No cores available for NUMA node {}", self.node_id));
            }

            // Запускаем по одному рабочему потоку на каждую RX очередь
            for queue_id in 0..num_rx_queues {
                // Выбираем CPU для этой очереди (round-robin)
                let core_idx = (queue_id as usize) % self.local_cpus.len();
                let core_id = self.local_cpus[core_idx];

                println!("  Queue {} -> Core {}", queue_id, core_id.id);

                // Запускаем рабочий поток
                let worker = self.start_worker_thread(
                    port_id,
                    queue_id,
                    core_id,
                    packet_handler.clone(),
                    packet_pool.clone(),
                    burst_size,
                );

                self.workers.push(worker);
            }
        }

        println!(
            "Started {} worker threads on NUMA node {}",
            self.workers.len(),
            self.node_id
        );
        Ok(())
    }

    /// Запускает рабочий поток
    fn start_worker_thread(
        &self,
        port_id: u16,
        queue_id: u16,
        core_id: CoreId,
        packet_handler: PacketHandler,
        packet_pool: Arc<PacketDataPool>,
        burst_size: u32,
    ) -> Worker {
        let running = self.running.clone();
        let node_id = self.node_id;

        // Создаем поток с заданным ядром процессора
        let thread = thread::spawn(move || {
            // Привязываем поток к ядру
            core_affinity::set_for_current(core_id);

            // Привязываем поток к узлу NUMA
            if NumaAllocator::is_available() {
                NumaAllocator::bind_thread_to_node(node_id);
                println!(
                    "Thread for port {}, queue {} bound to NUMA node {} core {}",
                    port_id, queue_id, node_id, core_id.id
                );
            }

            // DPDK rx_burst буфер
            let mut rx_pkts = vec![std::ptr::null_mut(); burst_size as usize];

            // Основной цикл обработки
            while running.load(Ordering::SeqCst) {
                // Вызов DPDK rx_burst
                let nb_rx = unsafe {
                    crate::dpdk::ffi::rte_eth_rx_burst(
                        port_id,
                        queue_id,
                        rx_pkts.as_mut_ptr(),
                        burst_size as u16,
                    )
                };

                for i in 0..nb_rx as usize {
                    let pkt = rx_pkts[i];

                    // Извлекаем данные пакета
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
                            pkt,
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
                        // Получаем пакет из пула
                        let mut packet = packet_pool.acquire();

                        // Заполняем его данными
                        packet.source_port = src_port;
                        packet.dest_port = dst_port;
                        packet.queue_id = queue_id;
                        packet.source_ip_ptr = src_ip_ptr;
                        packet.source_ip_len = src_ip_len as usize;
                        packet.dest_ip_ptr = dst_ip_ptr;
                        packet.dest_ip_len = dst_ip_len as usize;
                        packet.data_ptr = data_ptr;
                        packet.data_len = data_len as usize;
                        packet.mbuf_ptr = pkt;

                        // Обрабатываем пакет
                        packet_handler(queue_id, &packet);

                        // Освобождаем буфер пакета DPDK
                        unsafe { crate::dpdk::ffi::rte_pktmbuf_free(packet.mbuf_ptr) };

                        // Возвращаем пакет в пул
                        packet_pool.release(packet);
                    } else {
                        // В случае ошибки извлечения данных, освобождаем пакет
                        unsafe { crate::dpdk::ffi::rte_pktmbuf_free(pkt) };
                    }
                }
            }
        });

        Worker {
            thread: Some(thread),
            core_id,
            port_id,
            queue_id,
        }
    }

    /// Останавливает рабочие потоки
    pub fn stop_workers(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        println!(
            "Stopping {} worker threads on NUMA node {}",
            self.workers.len(),
            self.node_id
        );

        // Сигнал всем потокам остановиться
        self.running.store(false, Ordering::SeqCst);

        // Ждем завершения всех потоков
        while let Some(mut worker) = self.workers.pop() {
            if let Some(thread) = worker.thread.take() {
                let _ = thread.join();
                println!(
                    "  Worker thread for port {}, queue {} on core {} stopped",
                    worker.port_id, worker.queue_id, worker.core_id.id
                );
            }
        }
    }

    /// Генерирует аргументы для DPDK EAL, относящиеся к этому узлу NUMA
    pub fn generate_eal_args(&self, dpdk_config: &DpdkConfig) -> Vec<String> {
        let mut args = Vec::new();

        // Если используем huge pages, добавляем аргументы для выделения памяти на узле
        if dpdk_config.use_huge_pages {
            let socket_mem = if NumaAllocator::is_available() {
                // Если NUMA доступна, выделяем память только на этом узле
                let mut mem_per_node = vec!["0".to_string(); NumaAllocator::get_node_count()];
                mem_per_node[self.node_id] = dpdk_config
                    .socket_mem
                    .as_ref()
                    .map_or_else(|| "1024".to_string(), |v| v[self.node_id].to_string());
                mem_per_node.join(",")
            } else {
                // Если NUMA недоступна, выделяем всю память на одном узле
                dpdk_config
                    .socket_mem
                    .as_ref()
                    .map_or_else(|| "1024".to_string(), |v| v[0].to_string())
            };

            args.push(format!("--socket-mem={}", socket_mem));
        }

        args
    }

    /// Генерирует маску ядер для DPDK EAL, содержащую только ядра этого узла
    pub fn generate_core_mask(&self) -> String {
        let mut mask: u64 = 0;

        for core in &self.local_cpus {
            if core.id < 64 {
                mask |= 1 << core.id;
            }
        }

        format!("0x{:x}", mask)
    }
}

impl Drop for NumaNode {
    fn drop(&mut self) {
        self.stop_workers();
    }
}
