import { useCallback, useEffect, useRef, useState } from "react";
import { securityDiagnosticsContent as content } from "../../content/security-diagnostics.zh-CN";
import {
  getSecurityDiagnostics,
  type SecurityDiagnosticsV1,
} from "../../lib/ipc/security-diagnostics";
import { formatSecurityDiagnostics } from "./security-diagnostics-text";

export function SecurityDiagnosticsDialog({ onClose }: { onClose: () => void }) {
  const [diagnostics, setDiagnostics] = useState<SecurityDiagnosticsV1 | null>(null);
  const [failed, setFailed] = useState(false);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const mountedRef = useRef(true);

  const retry = useCallback(() => {
    setFailed(false);
    setDiagnostics(null);
    void getSecurityDiagnostics()
      .then((value) => {
        if (mountedRef.current) setDiagnostics(value);
      })
      .catch(() => {
        if (mountedRef.current) setFailed(true);
      });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    let active = true;
    void getSecurityDiagnostics()
      .then((value) => {
        if (active) setDiagnostics(value);
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => {
      active = false;
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    closeButtonRef.current?.focus();
    const handleDialogKeys = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [href], [tabindex]:not([tabindex='-1'])",
      );
      if (!focusable?.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const activeElement = document.activeElement;
      const activeIsFocusable = Array.from(focusable).some((element) => element === activeElement);
      if (event.shiftKey && (!activeIsFocusable || activeElement === first)) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && (!activeIsFocusable || activeElement === last)) {
        event.preventDefault();
        first?.focus();
      }
    };
    window.addEventListener("keydown", handleDialogKeys);
    return () => window.removeEventListener("keydown", handleDialogKeys);
  }, [onClose]);

  return (
    <div className="security-dialog-backdrop" role="presentation">
      <section
        aria-describedby="security-dialog-description"
        aria-labelledby="security-dialog-title"
        aria-modal="true"
        className="security-dialog"
        ref={dialogRef}
        role="dialog"
      >
        <header className="security-dialog-header">
          <div>
            <h2 id="security-dialog-title">{content.title}</h2>
            <p id="security-dialog-description">{content.introduction}</p>
          </div>
          <button ref={closeButtonRef} type="button" onClick={onClose}>
            {content.close}
          </button>
        </header>
        {diagnostics ? (
          <pre aria-label={content.selectableText} className="security-diagnostics-text">
            {formatSecurityDiagnostics(diagnostics)}
          </pre>
        ) : failed ? (
          <div className="security-dialog-message" role="alert">
            <p>{content.loadFailed}</p>
            <button type="button" onClick={retry}>
              {content.retry}
            </button>
          </div>
        ) : (
          <p className="security-dialog-message" role="status">
            {content.loading}
          </p>
        )}
      </section>
    </div>
  );
}
