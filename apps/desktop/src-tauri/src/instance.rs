//! Una sola acta por carpeta de datos. Sin guardia, un segundo proceso (la
//! app instalada y una de desarrollo, o un relanzamiento tras actualizar)
//! crea SU icono en la barra de menús: dos iconos de acta a la vez.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Candado exclusivo sobre `acta.lock`; el sistema lo libera al morir el proceso.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Resultado de intentar tomar el candado.
#[derive(Debug)]
pub enum Acquire {
    /// Somos la única instancia.
    Owned(InstanceLock),
    /// Otra acta ya tiene el candado.
    AlreadyRunning,
    /// No se pudo abrir/bloquear el fichero (carpeta ilegible, sistema de
    /// ficheros sin flock): no es "otra acta", hay que arrancar igualmente.
    Unavailable,
}

impl InstanceLock {
    pub fn acquire(root: &Path) -> Acquire {
        let Ok(file) = File::options().create(true).truncate(false).write(true).open(root.join("acta.lock")) else {
            return Acquire::Unavailable;
        };
        // SAFETY: `flock` sobre un descriptor abierto que vive tanto como `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Acquire::Owned(Self { _file: file });
        }
        match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::WouldBlock => Acquire::AlreadyRunning,
            _ => Acquire::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_instance_cannot_take_the_lock_until_the_first_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let first = InstanceLock::acquire(dir.path());
        assert!(matches!(first, Acquire::Owned(_)));
        assert!(matches!(InstanceLock::acquire(dir.path()), Acquire::AlreadyRunning), "segunda instancia");
        drop(first);
        assert!(matches!(InstanceLock::acquire(dir.path()), Acquire::Owned(_)));
    }
}
