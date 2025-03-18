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

// Базовые типы данных для работы с DPDK
#[repr(C)]
pub struct RteMbuf {
    _private: [u8; 0], // Непрозрачный тип для взаимодействия с C через FFI (пакетный буфер DPDK)
}

#[repr(C)]
pub struct RteMempool {
    _private: [u8; 0], // Непрозрачный тип для взаимодействия с C через FFI (пул памяти DPDK)
}

#[repr(C)]
pub struct RteEthRssConf {
    pub rss_key: *mut u8, // Указатель на ключ хеширования RSS
    pub rss_key_len: u8,  // Длина ключа RSS (обычно 40 байт)
    pub rss_hf: u64,      // Флаги для выбора алгоритмов хеширования RSS
}

// Константы для настройки RSS хеширования
pub const ETH_RSS_IP: u64 = 0x1; // Хеширование по IP-адресам источника и назначения
pub const ETH_RSS_TCP: u64 = 0x2; // Хеширование по TCP-портам источника и назначения
pub const ETH_RSS_UDP: u64 = 0x4; // Хеширование по UDP-портам источника и назначения
pub const ETH_RSS_SCTP: u64 = 0x8; // Хеширование по SCTP-портам (редко используется в биржевом трафике)
pub const ETH_MQ_RX_RSS: u32 = 1; // Режим работы мультиочередей - RSS (не количество очередей!)
pub const ETH_RSS_NONFRAG_IPV4_TCP: u64 = 0x40; // Хеширование для нефрагментированных TCP пакетов IPv4
pub const ETH_RSS_NONFRAG_IPV4_UDP: u64 = 0x80; // Хеширование для нефрагментированных UDP пакетов IPv4
pub const ETH_RSS_L4_DST_ONLY: u64 = 0x200; // Хеширование только по порту назначения (полезно для биржевого трафика)
pub const ETH_RSS_L4_SRC_ONLY: u64 = 0x100; // Хеширование только по порту источника

// Основная структура конфигурации DPDK
#[repr(C)]
pub struct DpdkConfig {
    // Основные параметры конфигурации DPDK
    pub port_id: c_ushort, // Идентификатор сетевого порта (обычно 0 для первой карты)
    pub num_rx_queues: c_ushort, // Количество очередей приема (рекомендуется по числу ядер)
    pub num_tx_queues: c_ushort, // Количество очередей передачи
    pub promiscuous: bool, // Режим прослушивания всех пакетов (true - принимать все пакеты)
    pub rx_ring_size: c_uint, // Размер кольцевого буфера приема (в пакетах)
    pub tx_ring_size: c_uint, // Размер кольцевого буфера передачи (в пакетах)
    pub num_mbufs: c_uint, // Количество буферов памяти для пакетов
    pub mbuf_cache_size: c_uint, // Размер кэша буферов для каждого потока
    pub burst_size: c_uint, // Размер пакетного чтения (за один вызов)
    pub enable_rss: bool,  // Включить RSS (распределение пакетов между очередями)
    pub rss_hf: u64,       // Флаги для выбора полей хеширования RSS
    pub use_cpu_affinity: bool, // Привязка потоков к ядрам процессора
    pub rss_key: Option<Vec<u8>>, // Пользовательский ключ для RSS (для детерминированного распределения)
}

// Структура конфигурации Ethernet устройства DPDK
#[repr(C)]
pub struct RteEthConf {
    pub rxmode: RteEthRxMode,         // Настройки режима приема
    pub txmode: RteEthTxMode,         // Настройки режима передачи
    pub lpbk_mode: u32,               // Режим петлевого тестирования (loopback)
    pub rx_adv_conf: RteEthRxAdvConf, // Расширенные настройки приема (включая RSS)
    pub tx_adv_conf: RteEthTxAdvConf, // Расширенные настройки передачи
    pub dcb_capability_en: u32,       // Включение возможностей DCB
    pub fdir_conf: RteEthFdirConf,    // Настройки Flow Director
    pub intr_conf: RteEthIntrConf,    // Настройки прерываний
}

// Настройки режима приема
#[repr(C)]
pub struct RteEthRxMode {
    pub mq_mode: u32, // Режим мультиочереди (0 - одна очередь, 1 - RSS, 2 - DCB, и т.д.)
    pub max_rx_pkt_len: u32, // Максимальная длина принимаемого пакета
    pub split_hdr_size: u16, // Размер разделения заголовка
    pub offloads: u64, // Флаги аппаратного ускорения
}

// Настройки режима передачи
#[repr(C)]
pub struct RteEthTxMode {
    pub mq_mode: u32,  // Режим мультиочереди для передачи
    pub pvid: u16,     // ID виртуального порта
    pub offloads: u64, // Флаги аппаратного ускорения
}

// Расширенные настройки приема
#[repr(C)]
pub struct RteEthRxAdvConf {
    pub rss_conf: RteEthRssConf, // Настройки RSS
                                 // По умолчанию другие поля пустые
}

// Расширенные настройки передачи
#[repr(C)]
pub struct RteEthTxAdvConf {
    // По умолчанию все поля пустые
}

// Настройки Flow Director (для детального управления маршрутизацией пакетов)
#[repr(C)]
pub struct RteEthFdirConf {
    // По умолчанию все поля пустые
}

// Настройки прерываний
#[repr(C)]
pub struct RteEthIntrConf {
    // По умолчанию все поля пустые
}

/// Представление извлеченных данных пакета для обработки в Rust
#[derive(Debug)]
pub struct PacketData<'a> {
    pub source_ip: Cow<'a, str>, // IP-адрес источника (в текстовом виде)
    pub dest_ip: Cow<'a, str>,   // IP-адрес назначения (в текстовом виде)
    pub source_port: u16,        // Порт источника
    pub dest_port: u16,          // Порт назначения
    pub data: &'a [u8],          // Данные пакета (полезная нагрузка)
    pub queue_id: u16,           // Номер очереди, в которую был направлен пакет
}

/// Коды ошибок для DPDK операций
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DpdkError {
    Success = 0,         // Успешное выполнение операции
    InitError = 1,       // Ошибка инициализации DPDK
    PortConfigError = 2, // Ошибка настройки сетевого порта
    MemoryError = 3,     // Ошибка выделения памяти
    RunningError = 4,    // Ошибка во время работы
    NotInitialized = 5,  // DPDK не инициализирован
}

// Внешние функции из библиотеки DPDK, импортируемые через FFI
#[link(name = "rte_eal")]
#[link(name = "rte_mempool")]
#[link(name = "rte_mbuf")]
#[link(name = "rte_ethdev")]
extern "C" {
    // Функции инициализации среды выполнения DPDK
    fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int; // Инициализация среды DPDK
    fn rte_eal_cleanup() -> c_int; // Освобождение ресурсов DPDK

    // Функции для работы с пулами памяти пакетов
    fn rte_pktmbuf_pool_create(
        name: *const c_char,      // Имя пула
        n: c_uint,                // Количество буферов в пуле
        cache_size: c_uint,       // Размер кэша для каждого потока
        priv_size: c_ushort,      // Размер приватных данных
        data_room_size: c_ushort, // Размер области данных буфера
        socket_id: c_int,         // ID сокета NUMA (-1 для любого)
    ) -> *mut RteMempool;

    // Функции для работы с сетевыми портами
    fn rte_eth_dev_is_valid_port(port_id: c_ushort) -> c_int; // Проверка существования порта
    fn rte_eth_dev_configure(
        port_id: c_ushort,       // ID порта
        nb_rx_queue: c_ushort,   // Количество очередей приема
        nb_tx_queue: c_ushort,   // Количество очередей передачи
        eth_conf: *const c_void, // Указатель на структуру конфигурации
    ) -> c_int;
    fn rte_eth_rx_queue_setup(
        port_id: c_ushort,        // ID порта
        rx_queue_id: c_ushort,    // ID очереди приема
        nb_rx_desc: c_ushort,     // Размер кольцевого буфера (количество дескрипторов)
        socket_id: c_int,         // ID сокета NUMA
        rx_conf: *const c_void,   // Указатель на конфигурацию очереди
        mb_pool: *mut RteMempool, // Пул буферов для очереди
    ) -> c_int;
    fn rte_eth_tx_queue_setup(
        port_id: c_ushort,      // ID порта
        tx_queue_id: c_ushort,  // ID очереди передачи
        nb_tx_desc: c_ushort,   // Размер кольцевого буфера
        socket_id: c_int,       // ID сокета NUMA
        tx_conf: *const c_void, // Указатель на конфигурацию очереди
    ) -> c_int;
    fn rte_eth_dev_start(port_id: c_ushort) -> c_int; // Запуск порта
    fn rte_eth_promiscuous_enable(port_id: c_ushort) -> c_int; // Включение прослушивания всех пакетов
    fn rte_eth_dev_stop(port_id: c_ushort) -> c_int; // Остановка порта
    fn rte_eth_dev_close(port_id: c_ushort) -> c_int; // Закрытие порта

    // Функции для работы с пакетами
    fn rte_eth_rx_burst(
        port_id: c_ushort,          // ID порта
        queue_id: c_ushort,         // ID очереди
        rx_pkts: *mut *mut RteMbuf, // Буфер для указателей на пакеты
        nb_pkts: c_ushort,          // Максимальное количество пакетов для чтения
    ) -> c_ushort; // Возвращает количество прочитанных пакетов
    fn rte_eth_tx_burst(
        port_id: c_ushort,          // ID порта
        queue_id: c_ushort,         // ID очереди
        tx_pkts: *mut *mut RteMbuf, // Буфер указателей на пакеты для отправки
        nb_pkts: c_ushort,          // Количество пакетов для отправки
    ) -> c_ushort; // Возвращает количество отправленных пакетов

    // Функции для работы с буферами пакетов
    fn rte_pktmbuf_free(m: *mut RteMbuf); // Освобождение буфера пакета
    fn rte_pktmbuf_mtod(m: *const RteMbuf, t: *const c_void) -> *mut c_void; // Получение указателя на данные пакета
    fn rte_pktmbuf_data_len(m: *const RteMbuf) -> c_ushort; // Получение длины данных в пакете

    // Наши собственные C-функции (из внешней библиотеки dpdk_helpers.c)
    fn dpdk_extract_packet_data(
        pkt: *const RteMbuf,         // Указатель на буфер пакета
        src_ip_out: *mut c_char,     // Выходной буфер для IP-адреса источника
        dst_ip_out: *mut c_char,     // Выходной буфер для IP-адреса назначения
        src_port_out: *mut c_ushort, // Выходная переменная для порта источника
        dst_port_out: *mut c_ushort, // Выходная переменная для порта назначения
        data_out: *mut *mut u8,      // Выходной указатель на данные пакета
        data_len_out: *mut c_uint,   // Выходная переменная для длины данных
    ) -> c_int; // Возвращает 0 при успехе, код ошибки иначе
}

/// Основная обёртка для работы с DPDK
pub struct DpdkWrapper {
    config: DpdkConfig,                  // Конфигурация DPDK
    mbuf_pool: *mut RteMempool,          // Пул памяти для пакетов
    initialized: bool,                   // Флаг инициализации
    running: Arc<AtomicBool>,            // Атомарный флаг работы (для безопасного завершения)
    worker_threads: Vec<JoinHandle<()>>, // Список рабочих потоков
}

/// Тип функции-обработчика для полученных пакетов (устарел)
pub type PacketHandler = Box<dyn Fn(&PacketData) + Send + 'static>;

/// Тип функции-обработчика пакетов с учетом номера очереди (рекомендуется использовать)
pub type QueueSpecificHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

impl DpdkWrapper {
    /// Создает новый экземпляр обёртки DPDK с указанной конфигурацией
    pub fn new(config: DpdkConfig) -> Self {
        DpdkWrapper {
            config,
            mbuf_pool: ptr::null_mut(),
            initialized: false,
            running: Arc::new(AtomicBool::new(false)),
            worker_threads: Vec::new(),
        }
    }

    /// Инициализирует DPDK с заданными аргументами командной строки
    pub fn init(&mut self, args: &[String]) -> Result<(), DpdkError> {
        if self.initialized {
            return Ok(());
        }

        // Конвертируем Rust-строки в массив указателей на C-строки для передачи в DPDK
        let c_args: Vec<CString> = args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();

        let mut c_argv: Vec<*mut c_char> = c_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();

        // Вызываем функцию инициализации DPDK (основная точка входа в DPDK)
        let ret = unsafe { rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };

        if ret < 0 {
            return Err(DpdkError::InitError);
        }

        // Создаем пул пакетных буферов (основной компонент DPDK для хранения пакетов)
        let pool_name = CString::new("mbuf_pool").unwrap();
        self.mbuf_pool = unsafe {
            rte_pktmbuf_pool_create(
                pool_name.as_ptr(),
                self.config.num_mbufs,
                self.config.mbuf_cache_size,
                0,  // priv_size - размер приватных данных (не используем)
                0,  // data_room_size - использовать значение по умолчанию (обычно 2048 байт)
                -1, // socket_id - любой сокет NUMA (оптимально указать конкретный для производительности)
            )
        };

        if self.mbuf_pool.is_null() {
            unsafe { rte_eal_cleanup() };
            return Err(DpdkError::MemoryError);
        }

        self.initialized = true;
        Ok(())
    }

    /// Настраивает сетевой порт для работы с указанной конфигурацией
    pub fn configure_port(&self) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id = self.config.port_id;

        // Проверяем, что указанный порт существует и доступен
        let is_valid = unsafe { rte_eth_dev_is_valid_port(port_id) };
        if is_valid == 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Создаем и инициализируем структуру конфигурации Ethernet
        let mut eth_conf = default_eth_config();

        // Настраиваем RSS только если включен и у нас больше одной очереди
        let enable_rss = self.config.enable_rss && self.config.num_rx_queues > 1;
        if enable_rss {
            // Устанавливаем режим мультиочереди на RSS
            eth_conf.rxmode.mq_mode = ETH_MQ_RX_RSS;

            // Устанавливаем оптимизированные настройки хеширования для биржевого трафика
            // Используем непосредственно оптимизированный набор флагов, избегая избыточности
            eth_conf.rx_adv_conf.rss_conf.rss_hf = self.config.rss_hf;

            // Устанавливаем пользовательский ключ RSS если он предоставлен
            if let Some(ref key) = self.config.rss_key {
                eth_conf.rx_adv_conf.rss_conf.rss_key = key.as_ptr() as *mut u8;
                eth_conf.rx_adv_conf.rss_conf.rss_key_len = key.len() as u8;
            }
        }

        // Настраиваем сетевое устройство с нашей правильно настроенной структурой
        let ret = unsafe {
            rte_eth_dev_configure(
                port_id,
                self.config.num_rx_queues,
                self.config.num_tx_queues,
                &eth_conf as *const RteEthConf as *const c_void,
            )
        };

        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Настраиваем каждую очередь приема (RX)
        for q in 0..self.config.num_rx_queues {
            let ret = unsafe {
                rte_eth_rx_queue_setup(
                    port_id,
                    q,
                    self.config.rx_ring_size as c_ushort,
                    -1,             // socket_id - любой сокет NUMA
                    ptr::null(),    // rx_conf - использовать настройки по умолчанию
                    self.mbuf_pool, // Пул буферов для этой очереди
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Настраиваем каждую очередь передачи (TX)
        for q in 0..self.config.num_tx_queues {
            let ret = unsafe {
                rte_eth_tx_queue_setup(
                    port_id,
                    q,
                    self.config.tx_ring_size as c_ushort,
                    -1,          // socket_id - любой сокет NUMA
                    ptr::null(), // tx_conf - использовать настройки по умолчанию
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Запускаем сетевой порт (активация)
        let ret = unsafe { rte_eth_dev_start(port_id) };
        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Включаем прослушивание всех пакетов (promiscuous mode), если это требуется
        if self.config.promiscuous {
            let ret = unsafe { rte_eth_promiscuous_enable(port_id) };
            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        Ok(())
    }

    /// Запускает обработку пакетов с указанными обработчиками для каждой очереди
    pub fn start_processing_with_queue_handlers(
        &mut self,
        queue_handlers: QueueSpecificHandler, // Функция обработчик для каждой очереди
    ) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id: u16 = self.config.port_id;
        let burst_size: u32 = self.config.burst_size;
        let running: Arc<AtomicBool> = self.running.clone();
        let num_queues: u16 = self.config.num_rx_queues;
        let use_affinity: bool = self.config.use_cpu_affinity;

        // Устанавливаем флаг, что обработка запущена
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

            // Создаем поток для обработки очереди с ID = queue_id
            let thread_handle: JoinHandle<()> = std::thread::spawn(move || {
                // Привязываем поток к ядру процессора, если это требуется и есть доступные ядра
                if use_affinity && !core_ids_clone.is_empty() {
                    let core_index: usize = (queue_id as usize) % core_ids_clone.len();
                    if let Some(core_id) = core_ids_clone.get(core_index) {
                        core_affinity::set_for_current(core_id.clone());
                    }
                }

                // Буфер для указателей на пакеты (для пакетного чтения)
                let mut rx_pkts: Vec<*mut RteMbuf> = vec![ptr::null_mut(); burst_size as usize];

                // Буферы для извлечения данных из пакетов
                let src_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для IP-адреса источника
                let dst_ip_buf: Vec<u8> = vec![0u8; 64]; // Буфер для IP-адреса назначения

                // Основной цикл обработки пакетов
                while running_clone.load(Ordering::SeqCst) {
                    // Получаем пакеты из текущей очереди (пакетное чтение)
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

                        // Вызываем C-функцию для извлечения данных из пакета
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
                            // Преобразуем C-строки IP-адресов в Rust-строки
                            let src_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(src_ip_ptr) }.to_string_lossy();
                            let dst_ip: Cow<'_, str> =
                                unsafe { CStr::from_ptr(dst_ip_ptr) }.to_string_lossy();

                            // Создаем ссылку на данные пакета без копирования (zero-copy)
                            let data: &[u8] =
                                unsafe { slice::from_raw_parts(data_ptr, data_len as usize) };

                            // Создаем структуру данных пакета для обработки в Rust-коде
                            let packet_data: PacketData<'_> = PacketData {
                                source_ip: src_ip,
                                dest_ip: dst_ip,
                                source_port: src_port,
                                dest_port: dst_port,
                                data,
                                queue_id: queue_id,
                            };

                            // Вызываем пользовательский обработчик пакета
                            queue_handler(queue_id, &packet_data);
                        }

                        // Освобождаем память пакета после обработки
                        unsafe { rte_pktmbuf_free(pkt) };
                    }
                }
            });

            // Сохраняем хэндл потока для последующего завершения
            self.worker_threads.push(thread_handle);
        }

        Ok(())
    }

    /// Останавливает обработку пакетов и ожидает завершения всех потоков
    pub fn stop(&mut self) {
        // Устанавливаем флаг остановки для всех потоков
        self.running.store(false, Ordering::SeqCst);

        // Ждем завершения всех рабочих потоков
        while let Some(handle) = self.worker_threads.pop() {
            let _ = handle.join();
        }
    }

    /// Освобождает ресурсы DPDK и останавливает обработку
    pub fn cleanup(&mut self) {
        if !self.initialized {
            return;
        }

        // Сначала останавливаем все потоки обработки
        self.stop();

        // Затем останавливаем и закрываем порт DPDK, освобождаем ресурсы
        unsafe {
            rte_eth_dev_stop(self.config.port_id);
            rte_eth_dev_close(self.config.port_id);
            rte_eal_cleanup();
        }

        self.initialized = false;
    }
}

// Автоматически освобождаем ресурсы при уничтожении объекта
impl Drop for DpdkWrapper {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn default_eth_config() -> RteEthConf {
    RteEthConf {
        rxmode: RteEthRxMode {
            mq_mode: 0,        // Будет установлено на ETH_MQ_RX_RSS, если RSS включен
            max_rx_pkt_len: 0, // Использовать значение по умолчанию
            split_hdr_size: 0, // Не разделять заголовки
            offloads: 0,       // Аппаратные ускорения отключены
        },
        txmode: RteEthTxMode {
            mq_mode: 0,  // Стандартный режим передачи
            pvid: 0,     // Не используем VLAN
            offloads: 0, // Аппаратные ускорения отключены
        },
        lpbk_mode: 0, // Режим петлевого тестирования выключен
        rx_adv_conf: RteEthRxAdvConf {
            rss_conf: RteEthRssConf {
                rss_key: ptr::null_mut(), // Ключ RSS будет установлен позже если требуется
                rss_key_len: 0,           // Длина ключа будет установлена позже
                rss_hf: 0,                // Функции хеширования будут установлены позже
            },
        },
        tx_adv_conf: RteEthTxAdvConf {}, // Стандартная конфигурация передачи
        dcb_capability_en: 0,            // DCB отключен
        fdir_conf: RteEthFdirConf {},    // Flow Director отключен
        intr_conf: RteEthIntrConf {},    // Стандартная конфигурация прерываний
    }
}

/// Создает оптимизированную конфигурацию DPDK с параметрами по умолчанию для биржевого приложения
pub fn default_dpdk_config() -> DpdkConfig {
    DpdkConfig {
        port_id: 0,           // Используем первый порт (0)
        num_rx_queues: 4,     // 4 очереди приема для современных сетевых карт
        num_tx_queues: 4,     // 4 очереди передачи
        promiscuous: true,    // Принимаем все пакеты (даже не адресованные нам)
        rx_ring_size: 1024,   // Размер кольцевого буфера приема (1024 пакета)
        tx_ring_size: 1024,   // Размер кольцевого буфера передачи
        num_mbufs: 8191,      // Количество буферов пакетов (обычно 2^n - 1)
        mbuf_cache_size: 250, // Размер кэша буферов для каждого потока
        burst_size: 32,       // Пакетное чтение по 32 пакета за вызов
        // Параметры RSS - оптимизировано для биржевого трафика
        enable_rss: true, // Включаем RSS для распределения нагрузки
        // Упрощенная и оптимизированная конфигурация хеширования для биржевого трафика:
        // Фокус на нефрагментированных пакетах и порте назначения
        rss_hf: ETH_RSS_NONFRAG_IPV4_TCP | ETH_RSS_NONFRAG_IPV4_UDP | ETH_RSS_L4_DST_ONLY,
        use_cpu_affinity: true, // Привязываем потоки к ядрам для лучшей производительности
        rss_key: None,          // Ключ RSS не указан (будет использован стандартный)
    }
}
