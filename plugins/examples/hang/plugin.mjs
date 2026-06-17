// Satisfies: S5 (DoS bound). Test fixture: run() never returns, so the host
// never sends a result. The Rust Session must TIME OUT and KILL this process
// rather than wait forever. See src-tauri/tests/sandbox.rs::hung_plugin_is_killed.
export async function run() {
  await new Promise(() => {}); // never resolves; does not spin the CPU
}
