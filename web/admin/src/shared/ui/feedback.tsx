import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

export type ToastKind = 'ok' | 'err' | 'info';

export type ToastAction = {
  label: string;
  onClick: () => void;
};

export type ToastOpts = {
  kind?: ToastKind;
  action?: ToastAction;
  durationMs?: number;
};

export type ConfirmOpts = {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  requireText?: string;
};

export type PromptOpts = {
  title: string;
  message: string;
  placeholder?: string;
  password?: boolean;
  confirmLabel?: string;
  cancelLabel?: string;
  allowEmpty?: boolean;
};

export type SecretOpts = {
  title: string;
  secret: string;
  hint?: string;
};

type ToastItem = {
  id: number;
  message: string;
  kind: ToastKind;
  action?: ToastAction;
};

type DialogState =
  | {
      kind: 'confirm';
      opts: ConfirmOpts;
      resolve: (ok: boolean) => void;
    }
  | {
      kind: 'prompt';
      opts: PromptOpts;
      resolve: (value: string | null) => void;
    }
  | {
      kind: 'secret';
      opts: SecretOpts;
      resolve: () => void;
    };

type FeedbackApi = {
  toast: (message: string, opts?: ToastOpts) => void;
  confirm: (opts: ConfirmOpts) => Promise<boolean>;
  prompt: (opts: PromptOpts) => Promise<string | null>;
  revealSecret: (opts: SecretOpts) => Promise<void>;
};

const FeedbackContext = createContext<FeedbackApi | null>(null);

let toastSeq = 0;

export function FeedbackProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const timers = useRef<Map<number, number>>(new Map());

  const dismissToast = useCallback((id: number) => {
    const t = timers.current.get(id);
    if (t) window.clearTimeout(t);
    timers.current.delete(id);
    setToasts((list) => list.filter((x) => x.id !== id));
  }, []);

  const toast = useCallback(
    (message: string, opts?: ToastOpts) => {
      const id = ++toastSeq;
      const kind = opts?.kind || 'info';
      const duration = opts?.durationMs ?? (kind === 'err' ? 6000 : 3800);
      setToasts((list) => [
        ...list.slice(-4),
        { id, message, kind, action: opts?.action },
      ]);
      if (duration > 0) {
        const handle = window.setTimeout(() => dismissToast(id), duration);
        timers.current.set(id, handle);
      }
    },
    [dismissToast],
  );

  const confirm = useCallback((opts: ConfirmOpts) => {
    return new Promise<boolean>((resolve) => {
      setDialog({ kind: 'confirm', opts, resolve });
    });
  }, []);

  const prompt = useCallback((opts: PromptOpts) => {
    return new Promise<string | null>((resolve) => {
      setDialog({ kind: 'prompt', opts, resolve });
    });
  }, []);

  const revealSecret = useCallback((opts: SecretOpts) => {
    return new Promise<void>((resolve) => {
      setDialog({ kind: 'secret', opts, resolve });
    });
  }, []);

  useEffect(() => {
    return () => {
      for (const t of timers.current.values()) window.clearTimeout(t);
      timers.current.clear();
    };
  }, []);

  const api = useMemo(
    () => ({ toast, confirm, prompt, revealSecret }),
    [toast, confirm, prompt, revealSecret],
  );

  return (
    <FeedbackContext.Provider value={api}>
      {children}
      <div className="toast-stack" aria-live="polite" aria-relevant="additions">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.kind}`} role="status">
            <span className="toast-msg">{t.message}</span>
            <div className="toast-actions">
              {t.action ? (
                <button
                  type="button"
                  className="ghost"
                  onClick={() => {
                    t.action?.onClick();
                    dismissToast(t.id);
                  }}
                >
                  {t.action.label}
                </button>
              ) : null}
              <button
                type="button"
                className="ghost"
                aria-label="关闭"
                onClick={() => dismissToast(t.id)}
              >
                ×
              </button>
            </div>
          </div>
        ))}
      </div>
      {dialog ? (
        <ModalDialog
          state={dialog}
          onClose={() => {
            if (dialog.kind === 'confirm') dialog.resolve(false);
            else if (dialog.kind === 'prompt') dialog.resolve(null);
            else dialog.resolve();
            setDialog(null);
          }}
          onDone={() => setDialog(null)}
        />
      ) : null}
    </FeedbackContext.Provider>
  );
}

export function useFeedback(): FeedbackApi {
  const ctx = useContext(FeedbackContext);
  if (!ctx) {
    throw new Error('useFeedback must be used within FeedbackProvider');
  }
  return ctx;
}

function ModalDialog({
  state,
  onClose,
  onDone,
}: {
  state: DialogState;
  onClose: () => void;
  onDone: () => void;
}) {
  const titleId = useId();
  const [text, setText] = useState('');
  const [copied, setCopied] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  useEffect(() => {
    if (state.kind === 'prompt' || state.kind === 'confirm') {
      queueMicrotask(() => inputRef.current?.focus());
    }
  }, [state.kind]);

  const requireText =
    state.kind === 'confirm' ? state.opts.requireText : undefined;
  const confirmDisabled = Boolean(
    requireText && text.trim() !== requireText,
  );

  async function copySecret(secret: string) {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
      >
        {state.kind === 'confirm' ? (
          <>
            <h3 id={titleId}>{state.opts.title}</h3>
            <p className="modal-body">{state.opts.message}</p>
            {requireText ? (
              <label className="modal-field">
                <span>
                  请输入 <code>{requireText}</code> 确认
                </span>
                <input
                  ref={inputRef}
                  value={text}
                  onChange={(e) => setText(e.target.value)}
                  placeholder={requireText}
                  autoComplete="off"
                />
              </label>
            ) : null}
            <div className="modal-actions">
              <button type="button" onClick={onClose}>
                {state.opts.cancelLabel || '取消'}
              </button>
              <button
                type="button"
                className={state.opts.danger ? 'danger' : 'primary'}
                disabled={confirmDisabled}
                onClick={() => {
                  state.resolve(true);
                  onDone();
                }}
              >
                {state.opts.confirmLabel || '确认'}
              </button>
            </div>
          </>
        ) : null}

        {state.kind === 'prompt' ? (
          <>
            <h3 id={titleId}>{state.opts.title}</h3>
            <p className="modal-body">{state.opts.message}</p>
            <label className="modal-field">
              <span className="sr-only">{state.opts.placeholder || '输入'}</span>
              <input
                ref={inputRef}
                type={state.opts.password ? 'password' : 'text'}
                value={text}
                placeholder={state.opts.placeholder}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    state.resolve(text);
                    onDone();
                  }
                }}
              />
            </label>
            <div className="modal-actions">
              <button type="button" onClick={onClose}>
                {state.opts.cancelLabel || '取消'}
              </button>
              <button
                type="button"
                className="primary"
                onClick={() => {
                  state.resolve(text);
                  onDone();
                }}
              >
                {state.opts.confirmLabel || '继续'}
              </button>
            </div>
          </>
        ) : null}

        {state.kind === 'secret' ? (
          <>
            <h3 id={titleId}>{state.opts.title}</h3>
            {state.opts.hint ? (
              <p className="modal-body warn-text">{state.opts.hint}</p>
            ) : (
              <p className="modal-body warn-text">
                仅显示一次，关闭后无法再次查看。请立即复制保存。
              </p>
            )}
            <pre className="secret-box">{state.opts.secret}</pre>
            <div className="modal-actions">
              <button
                type="button"
                className="primary"
                onClick={() => void copySecret(state.opts.secret)}
              >
                {copied ? '已复制' : '复制'}
              </button>
              <button
                type="button"
                onClick={() => {
                  state.resolve();
                  onDone();
                }}
              >
                我已保存
              </button>
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
