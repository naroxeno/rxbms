//! 后台解码线程池：worker 线程构造 `StaticSoundData`（kira 无后台解码 API，
//! 但 `from_file` 是纯 CPU 解码，可在工作线程执行；结果送回主线程缓存）。

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
};

use bevy::prelude::warn;
use kira::sound::static_sound::StaticSoundData;

/// 后台解码线程数上限（按 CPU 核数动态取 min，避免超线程无益开销）。
const MAX_DECODE_THREADS: usize = 4;

/// 结果通道容量：解码完成的 `StaticSoundData`（可能数十 MB）经有界通道回传，
/// 主线程消费慢时 worker 阻塞（背压），防止内存无限堆积。
const RESULT_CHANNEL_CAPACITY: usize = 64;

/// 后台解码线程池：worker 线程构造 `StaticSoundData`（kira 无后台解码 API，但
/// `from_file` 是纯 CPU 解码，可在工作线程执行；结果送回主线程缓存）。
///
/// 谱面剩余音频（首批之外）在此渐进解码，避免游玩中 `play_synced` 主线程现解卡顿。
pub(crate) struct DecodePool {
    tasks: Arc<Mutex<VecDeque<PathBuf>>>,
    condvar: Arc<Condvar>,
    /// 停止标志：`Drop` 时置位并唤醒全部 worker 退出（优雅关闭）。
    stop: Arc<AtomicBool>,
    #[allow(dead_code)] // 持有句柄保持线程存活（进程退出时随进程结束）
    _handles: Vec<thread::JoinHandle<()>>,
}

impl DecodePool {
    /// 启动线程池（worker 阻塞等待任务队列）。
    pub(crate) fn new(
        tx: mpsc::SyncSender<(PathBuf, Option<StaticSoundData>)>,
    ) -> Self {
        let tasks: Arc<Mutex<VecDeque<PathBuf>>> = Arc::new(Mutex::new(VecDeque::new()));
        let condvar = Arc::new(Condvar::new());
        let stop = Arc::new(AtomicBool::new(false));
        let workers = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(MAX_DECODE_THREADS);
        let mut handles = Vec::new();
        for _ in 0..workers {
            let tasks = tasks.clone();
            let condvar = condvar.clone();
            let stop = stop.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                // 取一个任务（空队列时阻塞等待）
                let path = {
                    let mut q = match tasks.lock() {
                        Ok(q) => q,
                        Err(_) => return,
                    };
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        // 空队列：等待唤醒（wait 原子释放锁，唤醒后重新拿锁）
                        match condvar.wait(q) {
                            Ok(q2) => q = q2,
                            Err(_) => return,
                        }
                    }
                };
                let result = match StaticSoundData::from_file(&path) {
                    Ok(data) => Some(data),
                    Err(e) => {
                        warn!("[audio] 后台解码失败 {}: {e}", path.display());
                        None
                    }
                };
                if tx.send((path, result)).is_err() {
                    return; // 接收端已销毁（应用退出）
                }
            }));
        }
        Self {
            tasks,
            condvar,
            stop,
            _handles: handles,
        }
    }

    /// 提交一个解码任务。
    pub(crate) fn submit(&self, path: PathBuf) {
        let mut q = match self.tasks.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        q.push_back(path);
        drop(q);
        self.condvar.notify_one();
    }
}

impl Drop for DecodePool {
    fn drop(&mut self) {
        // 优雅关闭：置停止标志并唤醒所有阻塞等待的 worker
        self.stop.store(true, Ordering::Relaxed);
        self.condvar.notify_all();
    }
}
