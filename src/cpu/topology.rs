// topology.rs - Improved implementation
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use core_affinity::CoreId;

#[derive(Debug, Clone)]
pub struct CpuTopology {
    pub total_cores: usize,
    pub physical_cores: usize,
    pub sockets: usize,
    /// Отображение логических ядер на физические
    /// Ключ: ID логического ядра, Значение: ID физического ядра
    pub core_mapping: HashMap<usize, usize>,
    /// Отображение логических ядер на сокеты
    /// Ключ: ID логического ядра, Значение: ID сокета
    pub socket_mapping: HashMap<usize, usize>,
    /// Список ядер, сгруппированных по физическим ядрам
    /// Ключ: ID физического ядра, Значение: Список ID логических ядер
    pub sibling_cores: HashMap<usize, Vec<usize>>,
    /// Список ядер, принадлежащих каждому сокету
    /// Ключ: ID сокета, Значение: Список ID логических ядер
    pub socket_cores: HashMap<usize, Vec<usize>>,
}

impl CpuTopology {
    pub fn new() -> io::Result<Self> {
        let mut topology = CpuTopology {
            total_cores: 0,
            physical_cores: 0,
            sockets: 0,
            core_mapping: HashMap::new(),
            socket_mapping: HashMap::new(),
            sibling_cores: HashMap::new(),
            socket_cores: HashMap::new(),
        };

        topology.load_topology()?;
        Ok(topology)
    }

    /// Загружает информацию о топологии процессора из системных файлов
    fn load_topology(&mut self) -> io::Result<()> {
        let cpu_path = Path::new("/sys/devices/system/cpu");

        let mut physical_cores = HashSet::new();
        let mut sockets = HashSet::new();

        for entry in fs::read_dir(cpu_path)? {
            let entry = entry?;
            let path = entry.path();
            let filename = path.file_name().unwrap().to_string_lossy();

            if !filename.starts_with("cpu") || !filename[3..].chars().all(char::is_numeric) {
                continue;
            }

            let cpu_id: usize = filename[3..].parse().unwrap_or(0);
            self.total_cores += 1;

            let topology_path = path.join("topology");

            if let Ok(core_id) = read_first_line(&topology_path.join("core_id")) {
                let core_id: usize = core_id.trim().parse().unwrap_or(0);
                self.core_mapping.insert(cpu_id, core_id);
                physical_cores.insert(core_id);

                self.sibling_cores
                    .entry(core_id)
                    .or_insert_with(Vec::new)
                    .push(cpu_id);
            }

            if let Ok(socket_id) = read_first_line(&topology_path.join("physical_package_id")) {
                let socket_id: usize = socket_id.trim().parse().unwrap_or(0);
                self.socket_mapping.insert(cpu_id, socket_id);
                sockets.insert(socket_id);

                self.socket_cores
                    .entry(socket_id)
                    .or_insert_with(Vec::new)
                    .push(cpu_id);
            }

            if let Ok(thread_siblings) =
                read_first_line(&topology_path.join("thread_siblings_list"))
            {
                let core_ids = parse_cpu_list(&thread_siblings);

                if !core_ids.is_empty() {
                    let phys_core_id = self.core_mapping.get(&cpu_id).unwrap_or(&cpu_id);
                    self.sibling_cores.insert(*phys_core_id, core_ids);
                }
            }
        }

        self.physical_cores = physical_cores.len();
        self.sockets = sockets.len();

        for cores in self.sibling_cores.values_mut() {
            cores.sort();
        }

        for cores in self.socket_cores.values_mut() {
            cores.sort();
        }

        Ok(())
    }

    /// Возвращает список ID первых логических ядер из каждой пары (без Hyper-Threading)
    pub fn get_physical_core_ids(&self) -> Vec<usize> {
        let mut result = Vec::new();

        for (physical_id, logical_ids) in &self.sibling_cores {
            if !logical_ids.is_empty() {
                let mut sorted_ids = logical_ids.clone();
                sorted_ids.sort();
                result.push(sorted_ids[0]);
            } else {
                result.push(*physical_id);
            }
        }

        result.sort();
        result
    }

    /// Возвращает список CoreId для core_affinity, исключая ядро 0
    pub fn get_filtered_core_ids(&self) -> Vec<CoreId> {
        let physical_ids = self.get_physical_core_ids();

        physical_ids
            .iter()
            .filter(|&&id| id != 0) // Исключаем ядро 0
            .map(|&id| CoreId { id })
            .collect()
    }

    /// Возвращает список CoreId для определенного NUMA-узла, исключая ядро 0 и HT-потоки
    pub fn get_socket_core_ids(&self, socket_id: usize) -> Vec<CoreId> {
        let physical_cores = self.get_physical_core_ids();

        match self.socket_cores.get(&socket_id) {
            Some(cores) => cores
                .iter()
                .filter(|&&id| physical_cores.contains(&id) && id != 0)
                .map(|&id| CoreId { id })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Возвращает все логические ядра для указанного NUMA-узла
    pub fn get_all_socket_cores(&self, socket_id: usize) -> Vec<usize> {
        match self.socket_cores.get(&socket_id) {
            Some(cores) => cores.clone(),
            None => Vec::new(),
        }
    }

    /// Генерирует маску процессора в формате, подходящем для аргументов DPDK EAL
    pub fn generate_core_mask(&self) -> String {
        let core_ids = self.get_filtered_core_ids();
        let mut mask: u64 = 0;

        for core in core_ids {
            if core.id < 64 {
                mask |= 1 << core.id;
            }
        }

        format!("0x{:x}", mask)
    }

    /// Генерирует список аргументов для DPDK EAL, включая маски процессоров
    pub fn generate_eal_cpu_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        let core_mask = self.generate_core_mask();
        args.push(format!("--lcores={}", core_mask));

        // Добавляем master-lcore, обычно это ядро 0
        args.push("--master-lcore=0".to_string());

        args
    }

    /// Возвращает ID сокета (NUMA-узла) для указанного ядра
    pub fn get_core_socket_id(&self, core_id: usize) -> Option<usize> {
        self.socket_mapping.get(&core_id).copied()
    }

    /// Проверяет, является ли указанное ядро первым логическим ядром в своей группе
    /// (т.е. не является ли оно HT-потоком)
    pub fn is_primary_logical_core(&self, core_id: usize) -> bool {
        if let Some(&physical_id) = self.core_mapping.get(&core_id) {
            if let Some(siblings) = self.sibling_cores.get(&physical_id) {
                if !siblings.is_empty() {
                    return siblings[0] == core_id;
                }
            }
        }

        // Если информации нет, предполагаем, что это первичное ядро
        true
    }

    /// Возвращает все доступные сокеты (NUMA-узлы)
    pub fn get_available_sockets(&self) -> Vec<usize> {
        let mut sockets: Vec<usize> = self.socket_cores.keys().cloned().collect();
        sockets.sort();
        sockets
    }

    /// Печатает информацию о топологии процессора для отладки
    pub fn print_topology_info(&self) {
        println!("CPU Topology Information:");
        println!("  Total logical cores: {}", self.total_cores);
        println!("  Physical cores: {}", self.physical_cores);
        println!("  Sockets (NUMA nodes): {}", self.sockets);

        println!("\nSocket mapping:");
        for socket_id in self.get_available_sockets() {
            let cores = self.get_all_socket_cores(socket_id);
            println!("  Socket {}: {:?}", socket_id, cores);

            // Показываем информацию о первичных логических ядрах для этого сокета
            let primary_cores: Vec<usize> = cores
                .iter()
                .filter(|&&id| self.is_primary_logical_core(id))
                .cloned()
                .collect();

            println!("    Primary logical cores: {:?}", primary_cores);
        }

        println!("\nPhysical to logical core mapping:");
        for (phys_id, logical_ids) in &self.sibling_cores {
            println!(
                "  Physical core {}: logical cores {:?}",
                phys_id, logical_ids
            );
        }

        println!(
            "\nFiltered core IDs (excluding HT and core 0): {:?}",
            self.get_filtered_core_ids()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        );
    }
}

impl fmt::Display for CpuTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "CPU Topology:")?;
        writeln!(f, "  Total cores: {}", self.total_cores)?;
        writeln!(f, "  Physical cores: {}", self.physical_cores)?;
        writeln!(f, "  Sockets: {}", self.sockets)?;

        writeln!(
            f,
            "  Filtered cores: {:?}",
            self.get_filtered_core_ids()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        )?;

        for socket_id in self.get_available_sockets() {
            writeln!(
                f,
                "  Socket {}: {} cores",
                socket_id,
                self.get_socket_core_ids(socket_id).len()
            )?;
        }

        Ok(())
    }
}

/// Чтение первой строки из файла
fn read_first_line<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    Ok(contents.lines().next().unwrap_or("").to_string())
}

/// Разбор списка процессоров из строки формата "0-3,5,7-9"
fn parse_cpu_list(list: &str) -> Vec<usize> {
    let mut result = Vec::new();

    for part in list.trim().split(',') {
        if part.contains('-') {
            let range: Vec<&str> = part.split('-').collect();
            if range.len() == 2 {
                if let (Ok(start), Ok(end)) = (range[0].parse::<usize>(), range[1].parse::<usize>())
                {
                    for i in start..=end {
                        result.push(i);
                    }
                }
            }
        } else if let Ok(num) = part.parse::<usize>() {
            result.push(num);
        }
    }

    result
}

/// Проверяет, доступна ли информация о топологии процессора
pub fn is_topology_info_available() -> bool {
    Path::new("/sys/devices/system/cpu/cpu0/topology").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology() {
        let topology = match CpuTopology::new() {
            Ok(t) => t,
            Err(e) => {
                println!("{e}");
                return;
            }
        };
        topology.print_topology_info();
    }

    #[test]
    fn test_info_available() {
        assert_eq!(is_topology_info_available(), true)
    }

    #[test]
    fn test_parse_cpu_list() {
        assert_eq!(parse_cpu_list("0-3,5,7-9"), vec![0, 1, 2, 3, 5, 7, 8, 9]);
        assert_eq!(parse_cpu_list("0,2,4"), vec![0, 2, 4]);
        assert_eq!(parse_cpu_list("0-2"), vec![0, 1, 2]);
    }
}
