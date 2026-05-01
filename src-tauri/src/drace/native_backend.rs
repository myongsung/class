use super::backends::{
  DraftVerifyResult, GenerationSessionApi, GenerationSessionHandle, GenerationSessionOptions,
  GenerationStepResult,
};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::raw::c_float;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(unix)]
const RTLD_NOW: c_int = 0x2;
#[cfg(unix)]
const RTLD_GLOBAL: c_int = 0x8;

#[cfg(unix)]
unsafe extern "C" {
  fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
  fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
  fn dlerror() -> *const c_char;
}

#[repr(C)]
struct LlamaModel;
#[repr(C)]
struct LlamaContext;
#[repr(C)]
struct LlamaVocab;

type LlamaToken = i32;
type LlamaPos = i32;
type LlamaSeqId = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct LlamaModelParams {
  devices: *mut c_void,
  n_gpu_layers: i32,
  split_mode: i32,
  main_gpu: i32,
  tensor_split: *const f32,
  progress_callback: *const c_void,
  progress_callback_user_data: *mut c_void,
  kv_overrides: *const c_void,
  vocab_only: bool,
  use_mmap: bool,
  use_mlock: bool,
  check_tensors: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LlamaContextParams {
  n_ctx: u32,
  n_batch: u32,
  n_ubatch: u32,
  n_seq_max: u32,
  n_threads: i32,
  n_threads_batch: i32,
  rope_scaling_type: i32,
  pooling_type: i32,
  attention_type: i32,
  rope_freq_base: f32,
  rope_freq_scale: f32,
  yarn_ext_factor: f32,
  yarn_attn_factor: f32,
  yarn_beta_fast: f32,
  yarn_beta_slow: f32,
  yarn_orig_ctx: u32,
  defrag_thold: f32,
  cb_eval: *const c_void,
  cb_eval_user_data: *mut c_void,
  type_k: i32,
  type_v: i32,
  embeddings: bool,
  offload_kqv: bool,
  flash_attn: bool,
  no_perf: bool,
  op_offload: bool,
  swa_full: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LlamaBatch {
  n_tokens: i32,
  token: *mut LlamaToken,
  embd: *mut f32,
  pos: *mut LlamaPos,
  n_seq_id: *mut i32,
  seq_id: *mut *mut LlamaSeqId,
  logits: *mut i8,
}

type FnLlamaBackendInit = unsafe extern "C" fn();
type FnLlamaBackendFree = unsafe extern "C" fn();
type FnLlamaModelDefaultParams = unsafe extern "C" fn() -> LlamaModelParams;
type FnLlamaContextDefaultParams = unsafe extern "C" fn() -> LlamaContextParams;
type FnLlamaModelLoadFromFile =
  unsafe extern "C" fn(path: *const c_char, params: LlamaModelParams) -> *mut LlamaModel;
type FnLlamaModelGetVocab = unsafe extern "C" fn(model: *mut LlamaModel) -> *const LlamaVocab;
type FnLlamaNewContextWithModel =
  unsafe extern "C" fn(model: *mut LlamaModel, params: LlamaContextParams) -> *mut LlamaContext;
type FnLlamaFreeModel = unsafe extern "C" fn(model: *mut LlamaModel);
type FnLlamaFree = unsafe extern "C" fn(ctx: *mut LlamaContext);
type FnLlamaTokenize = unsafe extern "C" fn(
  vocab: *const LlamaVocab,
  text: *const c_char,
  text_len: i32,
  tokens: *mut LlamaToken,
  n_tokens_max: i32,
  add_special: bool,
  parse_special: bool,
) -> i32;
type FnLlamaTokenToPiece = unsafe extern "C" fn(
  vocab: *const LlamaVocab,
  token: LlamaToken,
  buf: *mut c_char,
  length: i32,
  lstrip: i32,
  special: bool,
) -> i32;
type FnLlamaNToken = unsafe extern "C" fn(vocab: *const LlamaVocab) -> i32;
type FnLlamaBatchInit = unsafe extern "C" fn(n_tokens: i32, embd: i32, n_seq_max: i32) -> LlamaBatch;
type FnLlamaBatchFree = unsafe extern "C" fn(batch: LlamaBatch);
type FnLlamaDecode = unsafe extern "C" fn(ctx: *mut LlamaContext, batch: LlamaBatch) -> i32;
type FnLlamaGetLogits = unsafe extern "C" fn(ctx: *mut LlamaContext) -> *mut c_float;
type FnLlamaVocabEos = unsafe extern "C" fn(vocab: *const LlamaVocab) -> LlamaToken;
type FnLlamaTokenIsEog = unsafe extern "C" fn(vocab: *const LlamaVocab, token: LlamaToken) -> bool;

struct NativeLlamaApi {
  _lib_handle: *mut c_void,
  _dep_handles: Vec<*mut c_void>,
  llama_backend_init: FnLlamaBackendInit,
  llama_backend_free: FnLlamaBackendFree,
  llama_model_default_params: FnLlamaModelDefaultParams,
  llama_context_default_params: FnLlamaContextDefaultParams,
  llama_model_load_from_file: FnLlamaModelLoadFromFile,
  llama_model_get_vocab: FnLlamaModelGetVocab,
  llama_new_context_with_model: FnLlamaNewContextWithModel,
  llama_free_model: FnLlamaFreeModel,
  llama_free: FnLlamaFree,
  llama_tokenize: FnLlamaTokenize,
  llama_token_to_piece: FnLlamaTokenToPiece,
  llama_n_vocab: FnLlamaNToken,
  llama_batch_init: FnLlamaBatchInit,
  llama_batch_free: FnLlamaBatchFree,
  llama_decode: FnLlamaDecode,
  llama_get_logits: FnLlamaGetLogits,
  llama_vocab_eos: FnLlamaVocabEos,
  llama_token_is_eog: FnLlamaTokenIsEog,
}

unsafe impl Send for NativeLlamaApi {}
unsafe impl Sync for NativeLlamaApi {}

struct NativeLoadedModel {
  model: *mut LlamaModel,
  vocab: *const LlamaVocab,
}

unsafe impl Send for NativeLoadedModel {}
unsafe impl Sync for NativeLoadedModel {}

struct NativeSessionState {
  ctx: *mut LlamaContext,
  vocab: *const LlamaVocab,
  next_pos: i32,
  prompt_eval_ms: u128,
  prompt_tokens: usize,
}

unsafe impl Send for NativeSessionState {}
unsafe impl Sync for NativeSessionState {}

struct NativeBackendState {
  api: Arc<NativeLlamaApi>,
  models: HashMap<String, NativeLoadedModel>,
  sessions: HashMap<String, NativeSessionState>,
}

unsafe impl Send for NativeBackendState {}

static NATIVE_BACKEND_STATE: OnceLock<Result<Arc<Mutex<NativeBackendState>>, String>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct NativeSessionBackend;

pub fn native_verifier_available() -> Result<(), String> {
  resolve_native_llama_library_path().map(|_| ())
}

fn native_backend_state() -> Result<Arc<Mutex<NativeBackendState>>, String> {
  match NATIVE_BACKEND_STATE.get_or_init(|| {
    let api = Arc::new(load_native_llama_api()?);
    unsafe {
      (api.llama_backend_init)();
    }
    Ok(Arc::new(Mutex::new(NativeBackendState {
      api,
      models: HashMap::new(),
      sessions: HashMap::new(),
    })))
  }) {
    Ok(state) => Ok(state.clone()),
    Err(err) => Err(err.clone()),
  }
}

fn load_native_llama_api() -> Result<NativeLlamaApi, String> {
  let dep_handles = preload_native_dependencies()?;
  let lib_path = resolve_native_llama_library_path()?;
  let lib_handle = dlopen_path(&lib_path)?;
  unsafe {
    Ok(NativeLlamaApi {
      _lib_handle: lib_handle,
      _dep_handles: dep_handles,
      llama_backend_init: load_symbol(lib_handle, "llama_backend_init")?,
      llama_backend_free: load_symbol(lib_handle, "llama_backend_free")?,
      llama_model_default_params: load_symbol(lib_handle, "llama_model_default_params")?,
      llama_context_default_params: load_symbol(lib_handle, "llama_context_default_params")?,
      llama_model_load_from_file: load_symbol(lib_handle, "llama_model_load_from_file")?,
      llama_model_get_vocab: load_symbol(lib_handle, "llama_model_get_vocab")?,
      llama_new_context_with_model: load_symbol(lib_handle, "llama_new_context_with_model")?,
      llama_free_model: load_symbol(lib_handle, "llama_free_model")?,
      llama_free: load_symbol(lib_handle, "llama_free")?,
      llama_tokenize: load_symbol(lib_handle, "llama_tokenize")?,
      llama_token_to_piece: load_symbol(lib_handle, "llama_token_to_piece")?,
      llama_n_vocab: load_symbol(lib_handle, "llama_n_vocab")?,
      llama_batch_init: load_symbol(lib_handle, "llama_batch_init")?,
      llama_batch_free: load_symbol(lib_handle, "llama_batch_free")?,
      llama_decode: load_symbol(lib_handle, "llama_decode")?,
      llama_get_logits: load_symbol(lib_handle, "llama_get_logits")?,
      llama_vocab_eos: load_symbol(lib_handle, "llama_vocab_eos")?,
      llama_token_is_eog: load_symbol(lib_handle, "llama_token_is_eog")?,
    })
  }
}

fn resolve_native_llama_library_path() -> Result<PathBuf, String> {
  let mut candidates = Vec::new();
  let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  candidates.push(manifest.join("binaries").join("libllama.dylib"));
  candidates.push(manifest.join("binaries").join("libllama.0.dylib"));
  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      candidates.push(dir.join("libllama.dylib"));
      candidates.push(dir.join("sidecar").join("libllama.dylib"));
      candidates.push(dir.join("../Resources/sidecar/libllama.dylib"));
    }
  }
  for candidate in candidates {
    if candidate.exists() {
      return Ok(candidate);
    }
  }
  Err("native_verifier_library_missing".to_string())
}

fn preload_native_dependencies() -> Result<Vec<*mut c_void>, String> {
  let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  let binaries_dir = manifest.join("binaries");
  let allow_metal = matches!(
    std::env::var("ROOSYCOZY_NATIVE_VERIFIER_ENABLE_METAL").ok().as_deref(),
    Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on")
  );
  let names = [
    "libggml-base.dylib",
    "libggml.dylib",
    "libggml-cpu.dylib",
    "libggml-blas.dylib",
    "libggml-rpc.dylib",
    "libmtmd.dylib",
  ];
  let mut handles = Vec::new();
  for name in names {
    let candidate = binaries_dir.join(name);
    if candidate.exists() {
      handles.push(dlopen_path(&candidate)?);
    }
  }
  if allow_metal {
    let candidate = binaries_dir.join("libggml-metal.dylib");
    if candidate.exists() {
      handles.push(dlopen_path(&candidate)?);
    }
  }
  Ok(handles)
}

fn dlopen_path(path: &Path) -> Result<*mut c_void, String> {
  let c_path = CString::new(path.to_string_lossy().into_owned()).map_err(|e| e.to_string())?;
  #[cfg(unix)]
  unsafe {
    let handle = dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
    if handle.is_null() {
      return Err(last_dl_error().unwrap_or_else(|| format!("dlopen_failed: {}", path.display())));
    }
    Ok(handle)
  }
  #[cfg(not(unix))]
  {
    let _ = c_path;
    Err("native_verifier_unsupported_platform".to_string())
  }
}

#[cfg(unix)]
unsafe fn load_symbol<T>(handle: *mut c_void, symbol: &str) -> Result<T, String> {
  let c_symbol = CString::new(symbol).map_err(|e| e.to_string())?;
  let ptr = dlsym(handle, c_symbol.as_ptr());
  if ptr.is_null() {
    return Err(last_dl_error().unwrap_or_else(|| format!("missing_symbol: {symbol}")));
  }
  Ok(std::mem::transmute_copy(&ptr))
}

#[cfg(not(unix))]
unsafe fn load_symbol<T>(_handle: *mut c_void, symbol: &str) -> Result<T, String> {
  Err(format!("missing_symbol: {symbol}"))
}

#[cfg(unix)]
fn last_dl_error() -> Option<String> {
  unsafe {
    let err = dlerror();
    if err.is_null() {
      None
    } else {
      Some(CStr::from_ptr(err).to_string_lossy().to_string())
    }
  }
}

fn session_model_path(options: &GenerationSessionOptions) -> Result<&Path, String> {
  options
    .model_path
    .as_deref()
    .map(Path::new)
    .filter(|path| path.exists())
    .ok_or_else(|| "native_verifier_model_path_missing".to_string())
}

fn ensure_loaded_model<'a>(
  state: &'a mut NativeBackendState,
  model_path: &Path,
) -> Result<&'a NativeLoadedModel, String> {
  let key = model_path.to_string_lossy().into_owned();
  if !state.models.contains_key(&key) {
    let model = unsafe {
      let mut params = (state.api.llama_model_default_params)();
      params.n_gpu_layers = 0;
      params.use_mmap = true;
      params.use_mlock = false;
      let c_path = CString::new(key.clone()).map_err(|e| e.to_string())?;
      (state.api.llama_model_load_from_file)(c_path.as_ptr(), params)
    };
    if model.is_null() {
      return Err(format!("native_verifier_model_load_failed: {}", model_path.display()));
    }
    let vocab = unsafe { (state.api.llama_model_get_vocab)(model) };
    if vocab.is_null() {
      unsafe {
        (state.api.llama_free_model)(model);
      }
      return Err("native_verifier_vocab_missing".to_string());
    }
    state.models.insert(key.clone(), NativeLoadedModel { model, vocab });
  }
  state
    .models
    .get(&key)
    .ok_or_else(|| "native_verifier_model_cache_missing".to_string())
}

fn create_context(
  api: &NativeLlamaApi,
  model: &NativeLoadedModel,
  options: &GenerationSessionOptions,
) -> Result<*mut LlamaContext, String> {
  let mut params = unsafe { (api.llama_context_default_params)() };
  let n_ctx = options.n_ctx.unwrap_or(4096).clamp(2048, 8192);
  let n_threads = options.threads.unwrap_or(4).clamp(1, 6) as i32;
  params.n_ctx = n_ctx;
  params.n_batch = n_ctx.min(1024);
  params.n_ubatch = params.n_batch;
  params.n_seq_max = 1;
  params.n_threads = n_threads;
  params.n_threads_batch = n_threads;
  params.embeddings = false;
  params.offload_kqv = false;
  params.flash_attn = false;
  params.no_perf = false;
  let ctx = unsafe { (api.llama_new_context_with_model)(model.model, params) };
  if ctx.is_null() {
    Err("native_verifier_context_init_failed".to_string())
  } else {
    Ok(ctx)
  }
}

fn tokenize_text(api: &NativeLlamaApi, vocab: *const LlamaVocab, text: &str, add_special: bool) -> Result<Vec<LlamaToken>, String> {
  let c_text = CString::new(text).map_err(|e| e.to_string())?;
  let mut capacity = (text.chars().count().max(8) * 4 + 16) as i32;
  loop {
    let mut tokens = vec![0i32; capacity as usize];
    let written = unsafe {
      (api.llama_tokenize)(
        vocab,
        c_text.as_ptr(),
        text.len() as i32,
        tokens.as_mut_ptr(),
        capacity,
        add_special,
        false,
      )
    };
    if written >= 0 {
      tokens.truncate(written as usize);
      return Ok(tokens);
    }
    capacity = (-written).max(capacity * 2);
    if capacity > 32768 {
      return Err("native_verifier_tokenize_overflow".to_string());
    }
  }
}

fn decode_tokens(api: &NativeLlamaApi, ctx: *mut LlamaContext, tokens: &[LlamaToken], start_pos: i32) -> Result<(), String> {
  if tokens.is_empty() {
    return Ok(());
  }
  let batch = unsafe { (api.llama_batch_init)(tokens.len() as i32, 0, 1) };
  for (index, token) in tokens.iter().enumerate() {
    unsafe {
      *batch.token.add(index) = *token;
      *batch.pos.add(index) = start_pos + index as i32;
      *batch.n_seq_id.add(index) = 1;
      *(*batch.seq_id.add(index)) = 0;
      *batch.logits.add(index) = if index + 1 == tokens.len() { 1 } else { 0 };
    }
  }
  let decode_status = unsafe { (api.llama_decode)(ctx, batch) };
  unsafe {
    (api.llama_batch_free)(batch);
  }
  if decode_status != 0 {
    Err(format!("native_verifier_decode_failed: {decode_status}"))
  } else {
    Ok(())
  }
}

fn token_to_piece(api: &NativeLlamaApi, vocab: *const LlamaVocab, token: LlamaToken) -> Result<String, String> {
  let mut capacity = 64usize;
  loop {
    let mut buffer = vec![0u8; capacity];
    let written = unsafe {
      (api.llama_token_to_piece)(
        vocab,
        token,
        buffer.as_mut_ptr() as *mut c_char,
        capacity as i32,
        0,
        false,
      )
    };
    if written > 0 && (written as usize) <= buffer.len() {
      buffer.truncate(written as usize);
      return Ok(String::from_utf8_lossy(&buffer).to_string());
    }
    capacity *= 2;
    if capacity > 4096 {
      return Err("native_verifier_token_piece_overflow".to_string());
    }
  }
}

fn argmax_token(api: &NativeLlamaApi, ctx: *mut LlamaContext, vocab: *const LlamaVocab) -> Result<LlamaToken, String> {
  let logits = unsafe { (api.llama_get_logits)(ctx) };
  if logits.is_null() {
    return Err("native_verifier_logits_missing".to_string());
  }
  let n_vocab = unsafe { (api.llama_n_vocab)(vocab) };
  if n_vocab <= 0 {
    return Err("native_verifier_vocab_size_invalid".to_string());
  }
  let logits_slice = unsafe { slice::from_raw_parts(logits, n_vocab as usize) };
  let mut best_index = 0usize;
  let mut best_value = f32::NEG_INFINITY;
  for (index, value) in logits_slice.iter().enumerate() {
    if *value > best_value {
      best_value = *value;
      best_index = index;
    }
  }
  Ok(best_index as LlamaToken)
}

fn is_eog_token(api: &NativeLlamaApi, vocab: *const LlamaVocab, token: LlamaToken) -> bool {
  unsafe { (api.llama_token_is_eog)(vocab, token) || token == (api.llama_vocab_eos)(vocab) }
}

impl GenerationSessionApi for NativeSessionBackend {
  fn open_session(&self, prompt: &str, options: GenerationSessionOptions) -> Result<GenerationSessionHandle, String> {
    let state_lock = native_backend_state()?;
    let mut state = state_lock
      .lock()
      .map_err(|_| "native_verifier_state_poisoned".to_string())?;
    let model_path = session_model_path(&options)?.to_path_buf();
    let api = state.api.clone();
    let (model_ptr, vocab_ptr) = {
      let model = ensure_loaded_model(&mut state, &model_path)?;
      (model.model, model.vocab)
    };
    let temp_model = NativeLoadedModel {
      model: model_ptr,
      vocab: vocab_ptr,
    };
    let ctx = create_context(&api, &temp_model, &options)?;
    let mut full_prompt = prompt.to_string();
    if let Some(prefix) = options.assistant_prefix.as_deref() {
      if !prefix.is_empty() {
        full_prompt.push_str(prefix);
      }
    }
    let prompt_tokens = tokenize_text(&api, vocab_ptr, &full_prompt, true)?;
    let prompt_started = std::time::Instant::now();
    decode_tokens(&api, ctx, &prompt_tokens, 0)?;
    let prompt_eval_ms = prompt_started.elapsed().as_millis().max(1);
    let session_id = format!("native-session-{}-{}", options.model_id, crate::drace::fast_hash64(&full_prompt));
    state.sessions.insert(
      session_id.clone(),
      NativeSessionState {
        ctx,
        vocab: vocab_ptr,
        next_pos: prompt_tokens.len() as i32,
        prompt_eval_ms,
        prompt_tokens: prompt_tokens.len(),
      },
    );
    Ok(GenerationSessionHandle {
      session_id,
      backend_kind: super::cache_manager::BackendKind::Native,
      model_id: options.model_id.clone(),
      prompt: prompt.to_string(),
      options,
    })
  }

  fn generate_next(&self, session: &GenerationSessionHandle) -> Result<GenerationStepResult, String> {
    let started = std::time::Instant::now();
    let state_lock = native_backend_state()?;
    let mut state = state_lock
      .lock()
      .map_err(|_| "native_backend_state_poisoned".to_string())?;
    let api = state.api.clone();
    let session_state = state
      .sessions
      .get_mut(&session.session_id)
      .ok_or_else(|| "native_backend_session_missing".to_string())?;
    let max_tokens = session.options.max_tokens.max(1).min(2048) as usize;
    let mut generated = String::new();
    let mut generated_tokens = 0usize;
    let mut ttft_ms = None::<u128>;
    for _ in 0..max_tokens {
      let next_token = argmax_token(&api, session_state.ctx, session_state.vocab)?;
      if is_eog_token(&api, session_state.vocab, next_token) {
        break;
      }
      let piece = token_to_piece(&api, session_state.vocab, next_token)?;
      if piece.is_empty() {
        break;
      }
      if ttft_ms.is_none() {
        ttft_ms = Some(started.elapsed().as_millis().max(1));
      }
      decode_tokens(&api, session_state.ctx, &[next_token], session_state.next_pos)?;
      session_state.next_pos += 1;
      generated.push_str(&piece);
      generated_tokens += 1;
      if generated.ends_with("\n\n") && generated_tokens >= 32 {
        break;
      }
    }
    let decode_ms = started.elapsed().as_millis().max(1);
    Ok(GenerationStepResult {
      raw_response: serde_json::json!({
        "response": generated,
        "timings": {
          "prompt_ms": session_state.prompt_eval_ms as f64,
          "predicted_ms": decode_ms as f64,
        },
        "native": {
          "prompt_tokens": session_state.prompt_tokens,
          "output_tokens": generated_tokens,
        }
      }),
      response_started_ms: ttft_ms.unwrap_or(decode_ms),
    })
  }

  fn verify_draft(
    &self,
    session: &GenerationSessionHandle,
    proposed_token_ids: &[u32],
  ) -> Result<DraftVerifyResult, String> {
    let candidate_text = proposed_token_ids
      .iter()
      .filter_map(|id| char::from_u32(*id))
      .collect::<String>();
    if candidate_text.trim().is_empty() {
      return Ok(DraftVerifyResult::default());
    }
    let started = std::time::Instant::now();
    let state_lock = native_backend_state()?;
    let mut state = state_lock
      .lock()
      .map_err(|_| "native_verifier_state_poisoned".to_string())?;
    let api = state.api.clone();
    let session_state = state
      .sessions
      .get_mut(&session.session_id)
      .ok_or_else(|| "native_verifier_session_missing".to_string())?;
    let mut accepted_text = String::new();
    let mut accepted_tokens = Vec::new();
    loop {
      if accepted_text.len() >= candidate_text.len() {
        break;
      }
      let next_token = argmax_token(&api, session_state.ctx, session_state.vocab)?;
      let piece = token_to_piece(&api, session_state.vocab, next_token)?;
      if piece.is_empty() {
        break;
      }
      let next_text = format!("{}{}", accepted_text, piece);
      if !candidate_text.starts_with(&next_text) {
        let rejected_at = accepted_text.chars().count();
        return Ok(DraftVerifyResult {
          accepted_len: rejected_at,
          accepted_tokens,
          accepted_text: accepted_text.clone(),
          rejected_at: Some(rejected_at),
          verify_ms: started.elapsed().as_millis(),
          correction_token_id: Some(next_token as u32),
        });
      }
      decode_tokens(&api, session_state.ctx, &[next_token], session_state.next_pos)?;
      session_state.next_pos += 1;
      accepted_text = next_text;
      accepted_tokens.push(next_token as u32);
    }
    Ok(DraftVerifyResult {
      accepted_len: accepted_text.chars().count(),
      accepted_tokens,
      accepted_text,
      rejected_at: None,
      verify_ms: started.elapsed().as_millis(),
      correction_token_id: None,
    })
  }

  fn close_session(&self, session: &GenerationSessionHandle) -> Result<(), String> {
    let state_lock = native_backend_state()?;
    let mut state = state_lock
      .lock()
      .map_err(|_| "native_verifier_state_poisoned".to_string())?;
    if let Some(session_state) = state.sessions.remove(&session.session_id) {
      unsafe {
        (state.api.llama_free)(session_state.ctx);
      }
    }
    Ok(())
  }
}
