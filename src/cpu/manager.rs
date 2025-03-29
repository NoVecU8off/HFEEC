use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

use core_affinity::CoreId;
use num_cpus::get_physical;

use super::topology::{is_topology_info_available, CpuTopology};

/// Структура, предоставляющая функциональность для работы с процессором
#[derive(Debug)]
pub struct CpuManager {
    topology: CpuTopology,
    running: Arc<AtomicBool>,
    worker_threads: Vec<JoinHandle<()>>,
}

impl CpuManager {
    /// Создает новый экземпляр CpuManager
    pub fn new() -> Result<Self, std::io::Error> {
        let topology = CpuTopology::new()?;

        Ok(CpuManager {
            topology,
            running: Arc::new(AtomicBool::new(false)),
            worker_threads: Vec::new(),
        })
    }

    /// Возвращает ссылку на информацию о топологии процессора
    pub fn topology(&self) -> &CpuTopology {
        &self.topology
    }

    /// Возвращает список ID ядер, пригодных для использования (без HT и ядра 0)
    pub fn get_worker_core_ids(&self) -> Vec<CoreId> {
        self.topology.get_filtered_core_ids()
    }

    /// Возвращает список ID ядер, принадлежащих заданному сокету (NUMA-узлу)
    pub fn get_socket_core_ids(&self, socket_id: usize) -> Vec<CoreId> {
        self.topology.get_socket_core_ids(socket_id)
    }

    /// Запускает заданное количество рабочих потоков с привязкой к ядрам
    pub fn start_workers<F>(&mut self, worker_count: usize, worker_fn: F) -> usize
    where
        F: Fn(usize) + Send + Sync + 'static,
    {
        let core_ids = self.get_worker_core_ids();
        if core_ids.is_empty() {
            return 0;
        }

        let worker_fn = Arc::new(worker_fn);
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        // Ограничиваем количество рабочих потоков доступными ядрами
        let actual_worker_count = worker_count.min(core_ids.len());

        for worker_id in 0..actual_worker_count {
            let core_id = core_ids[worker_id % core_ids.len()];
            let worker_fn = worker_fn.clone();
            let running = running.clone();

            let handle = std::thread::spawn(move || {
                if !core_affinity::set_for_current(core_id) {
                    eprintln!("Failed to set core affinity for worker {}", worker_id);
                }

                let thread_id = worker_id;

                while running.load(Ordering::SeqCst) {
                    worker_fn(thread_id);
                }
            });

            self.worker_threads.push(handle);
        }

        actual_worker_count
    }

    /// Останавливает все рабочие потоки
    pub fn stop_workers(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        while let Some(handle) = self.worker_threads.pop() {
            let _ = handle.join();
        }
    }

    /// Генерирует список аргументов EAL для DPDK с настройками процессора
    pub fn generate_dpdk_eal_args(&self) -> Vec<String> {
        self.topology.generate_eal_cpu_args()
    }
}

/// Проверяет, включен ли Hyper-Threading
pub fn is_hyperthreading_enabled() -> bool {
    if let Ok(topology) = CpuTopology::new() {
        // Если количество логических ядер больше физических, значит HT включен
        topology.total_cores > topology.physical_cores
    } else {
        // Если не удалось получить информацию, предполагаем, что HT выключен
        false
    }
}

/// Проверяет доступность информации о топологии процессора
pub fn is_topology_available() -> bool {
    is_topology_info_available()
}

/// Возвращает рекомендуемое количество рабочих потоков
pub fn get_recommended_thread_count() -> usize {
    if let Ok(topology) = CpuTopology::new() {
        let filtered_cores = topology.get_filtered_core_ids().len();
        if filtered_cores > 0 {
            return filtered_cores;
        }
    }

    // Резервная логика, если не удалось определить через топологию
    let cores = get_physical();
    if cores > 1 {
        cores - 1
    } else {
        1
    }
}
