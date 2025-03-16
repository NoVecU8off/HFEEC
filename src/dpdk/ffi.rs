// ffi.rs
use std::borrow::Cow;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_ushort};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use core_affinity;

// Типы данных для работы с DPDK
#[repr(C)]
pub struct RteMbuf {
    _private: [u8; 0], // Непрозрачный тип для FFI
}

#[repr(C)]
pub struct RteMempool {
    _private: [u8; 0], // Непрозрачный тип для FFI
}

#[repr(C)]
pub struct RteEthRssConf {
    pub rss_key: *mut u8,
    pub rss_key_len: u8,
    pub rss_hf: u64, // Flags for RSS hash functions
}

// Константы для RSS хеширования
pub const ETH_RSS_IP: u64 = 0x1;
pub const ETH_RSS_TCP: u64 = 0x2;
pub const ETH_RSS_UDP: u64 = 0x4;
pub const ETH_RSS_SCTP: u64 = 0x8;

// Структуры для FFI
#[repr(C)]
pub struct DpdkConfig {
    // Основная конфигурация DPDK
    pub port_id: c_ushort,
    pub num_rx_queues: c_ushort,
    pub num_tx_queues: c_ushort,
    pub promiscuous: bool,
    pub rx_ring_size: c_uint,
    pub tx_ring_size: c_uint,
    pub num_mbufs: c_uint,
    pub mbuf_cache_size: c_uint,
    pub burst_size: c_uint,
    pub enable_rss: bool,
    pub rss_hf: u64,
    pub use_cpu_affinity: bool,
}

/// Представление пакета с данными
#[derive(Debug)]
pub struct PacketData<'a> {
    pub source_ip: Cow<'a, str>,
    pub dest_ip: Cow<'a, str>,
    pub source_port: u16,
    pub dest_port: u16,
    pub data: &'a [u8],
    pub queue_id: u16,
}

/// Коды ошибок
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpdkError {
    Success = 0,
    InitError = 1,
    PortConfigError = 2,
    MemoryError = 3,
    RunningError = 4,
    NotInitialized = 5,
}

// Внешние функции из библиотеки DPDK
#[link(name = "rte_eal")]
#[link(name = "rte_mempool")]
#[link(name = "rte_mbuf")]
#[link(name = "rte_ethdev")]
extern "C" {
    // Инициализация DPDK
    fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn rte_eal_cleanup() -> c_int;

    // Функции для работы с пулами памяти
    fn rte_pktmbuf_pool_create(
        name: *const c_char,
        n: c_uint,
        cache_size: c_uint,
        priv_size: c_ushort,
        data_room_size: c_ushort,
        socket_id: c_int,
    ) -> *mut RteMempool;

    // Функции для работы с портами
    fn rte_eth_dev_is_valid_port(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_configure(
        port_id: c_ushort,
        nb_rx_queue: c_ushort,
        nb_tx_queue: c_ushort,
        eth_conf: *const c_void,
    ) -> c_int;
    fn rte_eth_rx_queue_setup(
        port_id: c_ushort,
        rx_queue_id: c_ushort,
        nb_rx_desc: c_ushort,
        socket_id: c_int,
        rx_conf: *const c_void,
        mb_pool: *mut RteMempool,
    ) -> c_int;
    fn rte_eth_tx_queue_setup(
        port_id: c_ushort,
        tx_queue_id: c_ushort,
        nb_tx_desc: c_ushort,
        socket_id: c_int,
        tx_conf: *const c_void,
    ) -> c_int;
    fn rte_eth_dev_start(port_id: c_ushort) -> c_int;
    fn rte_eth_promiscuous_enable(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_stop(port_id: c_ushort) -> c_int;
    fn rte_eth_dev_close(port_id: c_ushort) -> c_int;

    // Функции для работы с пакетами
    fn rte_eth_rx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        rx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;
    fn rte_eth_tx_burst(
        port_id: c_ushort,
        queue_id: c_ushort,
        tx_pkts: *mut *mut RteMbuf,
        nb_pkts: c_ushort,
    ) -> c_ushort;

    // Функции для работы с буферами пакетов
    fn rte_pktmbuf_free(m: *mut RteMbuf);
    fn rte_pktmbuf_mtod(m: *const RteMbuf, t: *const c_void) -> *mut c_void;
    fn rte_pktmbuf_data_len(m: *const RteMbuf) -> c_ushort;

    // Наши собственные C-функции (имплементацию нужно будет написать)
    fn dpdk_extract_packet_data(
        pkt: *const RteMbuf,
        src_ip_out: *mut c_char,
        dst_ip_out: *mut c_char,
        src_port_out: *mut c_ushort,
        dst_port_out: *mut c_ushort,
        data_out: *mut *mut u8,
        data_len_out: *mut c_uint,
    ) -> c_int;
}

/// Обёртка для DPDK
pub struct DpdkWrapper {
    config: DpdkConfig,
    mbuf_pool: *mut RteMempool,
    initialized: bool,
    running: Arc<AtomicBool>,
    worker_threads: Vec<JoinHandle<()>>,
}

/// Тип колбека для обработки полученных данных
pub type PacketHandler = Box<dyn Fn(&PacketData) + Send + 'static>;

// Тип колбека для обработки пакетов с учетом очереди
pub type QueueSpecificHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

impl DpdkWrapper {
    /// Создает новый экземпляр обёртки DPDK
    pub fn new(config: DpdkConfig) -> Self {
        DpdkWrapper {
            config,
            mbuf_pool: ptr::null_mut(),
            initialized: false,
            running: Arc::new(AtomicBool::new(false)),
            worker_threads: Vec::new(),
        }
    }

    /// Инициализирует DPDK с заданными аргументами
    pub fn init(&mut self, args: &[String]) -> Result<(), DpdkError> {
        if self.initialized {
            return Ok(());
        }

        // Конвертируем Rust-строки в массив указателей на C-строки
        let c_args: Vec<CString> = args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();

        let mut c_argv: Vec<*mut c_char> = c_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();

        // Вызываем функцию инициализации DPDK
        let ret = unsafe { rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };

        if ret < 0 {
            return Err(DpdkError::InitError);
        }

        // Создаем пул пакетных буферов
        let pool_name = CString::new("mbuf_pool").unwrap();
        self.mbuf_pool = unsafe {
            rte_pktmbuf_pool_create(
                pool_name.as_ptr(),
                self.config.num_mbufs,
                self.config.mbuf_cache_size,
                0,  // priv_size
                0,  // data_room_size (использовать значение по умолчанию)
                -1, // socket_id (любой сокет)
            )
        };

        if self.mbuf_pool.is_null() {
            unsafe { rte_eal_cleanup() };
            return Err(DpdkError::MemoryError);
        }

        self.initialized = true;
        Ok(())
    }

    /// Настраивает сетевой порт
    pub fn configure_port(&self) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id = self.config.port_id;

        // Проверяем, что порт существует
        let is_valid = unsafe { rte_eth_dev_is_valid_port(port_id) };
        if is_valid == 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Создаем структуру конфигурации порта с RSS
        let mut eth_conf = Vec::<u8>::with_capacity(128); // Достаточный размер для rte_eth_conf
        eth_conf.resize(128, 0);
        let eth_conf_ptr = eth_conf.as_mut_ptr() as *mut c_void;

        if self.config.enable_rss {
            // Здесь должна быть детальная настройка RSS в структуре eth_conf
            // Из-за сложности FFI для DPDK структур, показываем псевдокод:
            //
            // eth_conf->rxmode.mq_mode = ETH_MQ_RX_RSS;
            // eth_conf->rx_adv_conf.rss_conf.rss_key = NULL;
            // eth_conf->rx_adv_conf.rss_conf.rss_key_len = 0;
            // eth_conf->rx_adv_conf.rss_conf.rss_hf = self.config.rss_hf;
        }

        // Настраиваем порт
        let ret = unsafe {
            rte_eth_dev_configure(
                port_id,
                self.config.num_rx_queues,
                self.config.num_tx_queues,
                eth_conf_ptr,
            )
        };

        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Настройка RX-очередей
        for q in 0..self.config.num_rx_queues {
            let ret = unsafe {
                rte_eth_rx_queue_setup(
                    port_id,
                    q,
                    self.config.rx_ring_size as c_ushort,
                    -1,          // socket_id (любой сокет)
                    ptr::null(), // rx_conf
                    self.mbuf_pool,
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Настройка TX-очередей
        for q in 0..self.config.num_tx_queues {
            let ret = unsafe {
                rte_eth_tx_queue_setup(
                    port_id,
                    q,
                    self.config.tx_ring_size as c_ushort,
                    -1,          // socket_id (любой сокет)
                    ptr::null(), // tx_conf
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Запуск порта
        let ret = unsafe { rte_eth_dev_start(port_id) };
        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Включаем прослушивание всех пакетов, если требуется
        if self.config.promiscuous {
            let ret = unsafe { rte_eth_promiscuous_enable(port_id) };
            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        Ok(())
    }

    pub fn start_packet_processing(&mut self, handler: PacketHandler) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;
        let burst_size: u32 = self.config.burst_size;
        let running: Arc<AtomicBool> = self.running.clone();
        let num_queues: u16 = self.config.num_rx_queues;
        let use_affinity: bool = self.config.use_cpu_affinity;

        // Устанавливаем флаг, что мы работаем
        running.store(true, Ordering::SeqCst);

        // Получаем список доступных ядер процессора для привязки потоков
        // Используем Arc для возможности безопасного совместного использования в разных потоках
        let core_ids = Arc::new(if use_affinity {
            core_affinity::get_core_ids().unwrap_or_default()
        } else {
            Vec::new()
        });

        // Создаем канал для передачи данных пакетов
        let (tx, rx) = std::sync::mpsc::channel();

        // Создаем отдельный поток для каждой очереди
        for queue_id in 0..num_queues {
            let tx_clone: std::sync::mpsc::Sender<PacketData<'_>> = tx.clone();
            let running_clone: Arc<AtomicBool> = running.clone();
            let core_ids_clone: Arc<Vec<core_affinity::CoreId>> = core_ids.clone();

            // Создаем поток для очереди queue_id
            let thread_handle: JoinHandle<()> = std::thread::spawn(move || {
                // Привязка потока к ядру, если требуется и есть доступные ядра
                if use_affinity && !core_ids_clone.is_empty() {
                    let core_index: usize = (queue_id as usize) % core_ids_clone.len();
                    if let Some(core_id) = core_ids_clone.get(core_index) {
                        core_affinity::set_for_current(core_id.clone());
                    }
                }

                // Буфер для указателей на пакеты
                let mut rx_pkts: Vec<*mut RteMbuf> = vec![ptr::null_mut(); burst_size as usize];

                // Буферы для извлечения данных из пакетов
                let src_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для source IP
                let dst_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для destination IP

                while running_clone.load(Ordering::SeqCst) {
                    // Получаем пакеты из этой очереди
                    let nb_rx: u16 = unsafe {
                        rte_eth_rx_burst(
                            port_id,
                            queue_id, // Используем ID текущей очереди
                            rx_pkts.as_mut_ptr(),
                            burst_size as c_ushort,
                        )
                    };

                    // Обрабатываем каждый полученный пакет
                    for i in 0..nb_rx as usize {
                        let pkt: *mut RteMbuf = rx_pkts[i];

                        // Извлекаем информацию о пакете через нашу вспомогательную C-функцию
                        let src_ip_ptr: *mut i8 = src_ip_buf.as_ptr() as *mut c_char;
                        let dst_ip_ptr: *mut i8 = dst_ip_buf.as_ptr() as *mut c_char;
                        let mut src_port: c_ushort = 0;
                        let mut dst_port: c_ushort = 0;
                        let mut data_ptr: *mut u8 = ptr::null_mut();
                        let mut data_len: c_uint = 0;

                        let ret: i32 = unsafe {
                            dpdk_extract_packet_data(
                                pkt,
                                src_ip_ptr,
                                dst_ip_ptr,
                                &mut src_port,
                                &mut dst_port,
                                &mut data_ptr,
                                &mut data_len,
                            )
                        };

                        // Если успешно извлекли данные из пакета
                        if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                            let src_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(src_ip_ptr) }.to_string_lossy();
                            let dst_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(dst_ip_ptr) }.to_string_lossy();

                            // Копируем данные пакета в Rust-вектор
                            let data: &[u8] =
                                unsafe { slice::from_raw_parts(data_ptr, data_len as usize) };

                            // Создаем структуру данных пакета (добавляем queue_id)
                            let packet_data: PacketData<'_> = PacketData {
                                source_ip: src_ip,
                                dest_ip: dst_ip,
                                source_port: src_port,
                                dest_port: dst_port,
                                data,
                                queue_id: queue_id,
                            };

                            // Отправляем данные через канал
                            let _ = tx_clone.send(packet_data);
                        }

                        // Освобождаем память пакета после обработки
                        unsafe { rte_pktmbuf_free(pkt) };
                    }
                }
            });

            // Сохраняем handle потока
            self.worker_threads.push(thread_handle);
        }

        // Поток для обработки данных пакетов
        let handler_thread: JoinHandle<()> = std::thread::spawn(move || {
            while let Ok(packet_data) = rx.recv() {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Вызываем обработчик
                handler(&packet_data);
            }
        });

        // Сохраняем handle потока обработчика
        self.worker_threads.push(handler_thread);

        Ok(())
    }

    pub fn start_processing_with_queue_handlers(
        &mut self,
        queue_handlers: QueueSpecificHandler,
    ) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;
        let burst_size: u32 = self.config.burst_size;
        let running: Arc<AtomicBool> = self.running.clone();
        let num_queues: u16 = self.config.num_rx_queues;
        let use_affinity: bool = self.config.use_cpu_affinity;

        // Устанавливаем флаг, что мы работаем
        running.store(true, Ordering::SeqCst);

        // Получаем список доступных ядер процессора для привязки потоков
        // Используем Arc для безопасного совместного использования между потоками
        let core_ids: Arc<Vec<core_affinity::CoreId>> = Arc::new(if use_affinity {
            core_affinity::get_core_ids().unwrap_or_default()
        } else {
            Vec::new()
        });

        // Для каждой очереди создаем отдельный поток обработки
        for queue_id in 0..num_queues {
            let queue_handler: Arc<dyn Fn(u16, &PacketData<'_>) + Send + Sync> =
                queue_handlers.clone();
            let running_clone: Arc<AtomicBool> = running.clone();
            let core_ids_clone: Arc<Vec<core_affinity::CoreId>> = core_ids.clone(); // Клонируем Arc, не Vec

            // Создаем поток для очереди queue_id
            let thread_handle: JoinHandle<()> = std::thread::spawn(move || {
                // Привязка потока к ядру, если требуется и есть доступные ядра
                if use_affinity && !core_ids_clone.is_empty() {
                    let core_index: usize = (queue_id as usize) % core_ids_clone.len();
                    if let Some(core_id) = core_ids_clone.get(core_index) {
                        core_affinity::set_for_current(core_id.clone());
                    }
                }

                // Буфер для указателей на пакеты
                let mut rx_pkts: Vec<*mut RteMbuf> = vec![ptr::null_mut(); burst_size as usize];

                // Буферы для извлечения данных из пакетов
                let src_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для source IP
                let dst_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для destination IP

                while running_clone.load(Ordering::SeqCst) {
                    // Получаем пакеты из этой очереди
                    let nb_rx: u16 = unsafe {
                        rte_eth_rx_burst(
                            port_id,
                            queue_id, // Используем ID текущей очереди
                            rx_pkts.as_mut_ptr(),
                            burst_size as c_ushort,
                        )
                    };

                    // Обрабатываем каждый полученный пакет
                    for i in 0..nb_rx as usize {
                        let pkt: *mut RteMbuf = rx_pkts[i];

                        // Извлекаем информацию о пакете через нашу вспомогательную C-функцию
                        let src_ip_ptr: *mut i8 = src_ip_buf.as_ptr() as *mut c_char;
                        let dst_ip_ptr: *mut i8 = dst_ip_buf.as_ptr() as *mut c_char;
                        let mut src_port: c_ushort = 0;
                        let mut dst_port: c_ushort = 0;
                        let mut data_ptr: *mut u8 = ptr::null_mut();
                        let mut data_len: c_uint = 0;

                        let ret: i32 = unsafe {
                            dpdk_extract_packet_data(
                                pkt,
                                src_ip_ptr,
                                dst_ip_ptr,
                                &mut src_port,
                                &mut dst_port,
                                &mut data_ptr,
                                &mut data_len,
                            )
                        };

                        // Если успешно извлекли данные из пакета
                        if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                            let src_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(src_ip_ptr) }.to_string_lossy();
                            let dst_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(dst_ip_ptr) }.to_string_lossy();

                            // Копируем данные пакета в Rust-вектор
                            let data: &[u8] =
                                unsafe { slice::from_raw_parts(data_ptr, data_len as usize) };

                            // Создаем структуру данных пакета
                            let packet_data: PacketData<'_> = PacketData {
                                source_ip: src_ip,
                                dest_ip: dst_ip,
                                source_port: src_port,
                                dest_port: dst_port,
                                data,
                                queue_id: queue_id,
                            };

                            // Вызываем обработчик напрямую
                            queue_handler(queue_id, &packet_data);
                        }

                        // Освобождаем память пакета после обработки
                        unsafe { rte_pktmbuf_free(pkt) };
                    }
                }
            });

            // Сохраняем handle потока
            self.worker_threads.push(thread_handle);
        }

        Ok(())
    }

    /// Останавливает обработку пакетов и ждет завершения потоков
    pub fn stop(&mut self) {
        // Устанавливаем флаг остановки
        self.running.store(false, Ordering::SeqCst);

        // Ждем завершения всех рабочих потоков
        while let Some(handle) = self.worker_threads.pop() {
            let _ = handle.join();
        }
    }

    /// Освобождает ресурсы DPDK
    pub fn cleanup(&mut self) {
        if !self.initialized {
            return;
        }

        self.stop();

        // Останавливаем порт
        unsafe {
            rte_eth_dev_stop(self.config.port_id);
            rte_eth_dev_close(self.config.port_id);
            rte_eal_cleanup();
        }

        self.initialized = false;
    }
}

impl Drop for DpdkWrapper {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Создает конфигурацию DPDK с параметрами по умолчанию (модифицированная)
pub fn default_dpdk_config() -> DpdkConfig {
    DpdkConfig {
        port_id: 0,
        num_rx_queues: 4, // Устанавливаем 4 очереди по умолчанию для современных NIC
        num_tx_queues: 4, // Устанавливаем 4 очереди по умолчанию для современных NIC
        promiscuous: true,
        rx_ring_size: 1024,
        tx_ring_size: 1024,
        num_mbufs: 8191,
        mbuf_cache_size: 250,
        burst_size: 32,
        // Новые параметры
        enable_rss: true,
        rss_hf: ETH_RSS_IP | ETH_RSS_TCP | ETH_RSS_UDP, // Хеширование по IP, TCP и UDP
        use_cpu_affinity: true, // Привязка потоков к ядрам процессора по умолчанию включена
    }
}
