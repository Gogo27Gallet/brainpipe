//! Python stream boundary and bounded native producer lifetime.
use crate::DocumentPage;
use pyo3::prelude::*;
use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

enum ChunkStreamBackend {
    Channel(Arc<Mutex<mpsc::Receiver<PyResult<DocumentPage>>>>),
    Buffer {
        pages: Vec<DocumentPage>,
        index: AtomicUsize,
    },
}

#[pyclass(unsendable)]
pub struct ChunkStream {
    backend: ChunkStreamBackend,
}

impl ChunkStream {
    pub(crate) fn from_channel(rx: mpsc::Receiver<PyResult<DocumentPage>>) -> Self {
        ChunkStream {
            backend: ChunkStreamBackend::Channel(Arc::new(Mutex::new(rx))),
        }
    }

    pub(crate) fn from_vec(pages: Vec<DocumentPage>) -> Self {
        ChunkStream {
            backend: ChunkStreamBackend::Buffer {
                pages,
                index: AtomicUsize::new(0),
            },
        }
    }
}

#[pymethods]
impl ChunkStream {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> { slf }

    /// Disconnect producers, including those waiting on the bounded channel.
    fn close(&mut self) {
        self.backend = ChunkStreamBackend::Buffer {
            pages: Vec::new(), index: AtomicUsize::new(0),
        };
    }

    /// Drain all pages in one Rust loop (much faster than repeated __next__ from Python).
    fn drain_all(mut slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<Vec<DocumentPage>> {
        match &mut slf.backend {
            ChunkStreamBackend::Buffer { pages, index } => {
                index.store(pages.len(), Ordering::Relaxed);
                Ok(std::mem::take(pages))
            }
            ChunkStreamBackend::Channel(rx) => {
                let rx = rx.clone();
                py.allow_threads(move || {
                    let lock = rx.lock().unwrap();
                    let mut out = Vec::new();
                    while let Ok(item) = lock.recv() {
                        out.push(item?);
                    }
                    Ok(out)
                })
            }
        }
    }

    #[allow(deprecated)]
    fn __next__(slf: PyRefMut<'_, Self>, py: Python) -> PyResult<Option<PyObject>> {
        match &slf.backend {
            ChunkStreamBackend::Buffer { pages, index } => {
                let i = index.fetch_add(1, Ordering::Relaxed);
                if i >= pages.len() {
                    return Ok(None);
                }
                return Ok(Some(pages[i].clone().into_py(py)));
            }
            ChunkStreamBackend::Channel(rx) => {
                let rx = rx.clone();
                py.allow_threads(move || {
                    let lock = rx.lock().unwrap();
                    lock.recv().ok()
                })
                .transpose().map(|doc| doc.map(|doc| doc.into_py(py)))
            }
        }
    }
}
