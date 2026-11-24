#![allow(clippy::unwrap_used, clippy::expect_used)]
//! El candado de instancia única entre DOS procesos de verdad (flock es por
//! descriptor abierto: un test en el mismo proceso no demuestra nada sobre
//! una segunda app). El propio binario de tests se relanza como "hijo" que
//! retiene el candado hasta que se cierra su stdin.

use acta_desktop_lib::instance::{Acquire, InstanceLock};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};

const DIR_VAR: &str = "ACTA_TEST_LOCK_DIR";

/// Hijo: toma el candado, avisa y espera a que le cierren stdin.
#[test]
fn child_holds_the_lock() {
    let Ok(dir) = std::env::var(DIR_VAR) else { return };
    let lock = InstanceLock::acquire(Path::new(&dir));
    println!("CHILD_{}", if matches!(lock, Acquire::Owned(_)) { "LOCKED" } else { "DENIED" });
    let mut sink = Vec::new();
    std::io::stdin().read_to_end(&mut sink).unwrap();
}

/// Mata y espera al hijo en todos los caminos (también si un assert falla).
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

#[allow(clippy::disallowed_methods)] // el ChildGuard garantiza kill() + wait()
fn spawn_child(dir: &Path) -> ChildGuard {
    ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["child_holds_the_lock", "--exact", "--nocapture", "--test-threads=1"])
            .env(DIR_VAR, dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

/// Devuelve el lector: hay que mantenerlo vivo o el hijo muere con SIGPIPE al escribir.
fn wait_for_line(child: &mut ChildGuard, wanted: &str) -> BufReader<std::process::ChildStdout> {
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap() > 0 {
        assert!(!line.contains("CHILD_DENIED"), "el hijo no pudo tomar el candado");
        if line.contains(wanted) {
            return reader;
        }
        line.clear();
    }
    panic!("el hijo terminó sin decir {wanted}");
}

#[test]
fn a_second_process_is_refused_while_the_first_lives_and_succeeds_after_it_dies() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn_child(dir.path());
    let _out = wait_for_line(&mut child, "CHILD_LOCKED");

    assert!(matches!(InstanceLock::acquire(dir.path()), Acquire::AlreadyRunning), "otro proceso lo tiene");

    // Muerte limpia (stdin cerrado) y muerte brusca (SIGKILL): el sistema libera el candado.
    drop(child.stdin.take());
    child.wait().unwrap();
    let again = InstanceLock::acquire(dir.path());
    assert!(matches!(again, Acquire::Owned(_)), "tras salir el hijo");
    drop(again);

    let mut killed = spawn_child(dir.path());
    let _out2 = wait_for_line(&mut killed, "CHILD_LOCKED");
    assert!(matches!(InstanceLock::acquire(dir.path()), Acquire::AlreadyRunning));
    killed.kill().unwrap();
    killed.wait().unwrap();
    assert!(matches!(InstanceLock::acquire(dir.path()), Acquire::Owned(_)), "tras SIGKILL");
}

/// Si la carpeta de datos no se puede abrir NO es "otra acta abierta": abrir
/// y cerrar en silencio dejaría al usuario sin app ni explicación.
#[test]
fn an_unusable_data_folder_is_not_reported_as_another_running_instance() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-existe");
    assert!(matches!(InstanceLock::acquire(&missing), Acquire::Unavailable));
}
