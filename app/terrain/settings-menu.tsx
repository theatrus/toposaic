"use client";

import {
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import { terrainApi } from "./api";
import { formatBytes } from "./config";
import type { CacheCategoryKey, CacheStats } from "./contracts";
import { useOutsideDismiss } from "./use-outside-dismiss";

const CACHE_CATEGORY_NAMES: Record<CacheCategoryKey, string> = {
  elevation: "Elevation tiles",
  world_cover: "Land cover",
  osm: "OpenStreetMap",
  datum: "Tide datums",
  places: "Place search",
};

const CLEAR_AGE_CHOICES = [7, 30, 90] as const;
const DEFAULT_CLEAR_AGE_DAYS = 30;

function entriesLabel(count: number) {
  return `${count} ${count === 1 ? "entry" : "entries"}`;
}

export function SettingsMenu() {
  const [open, setOpen] = useState(false);
  const [cache, setCache] = useState<
    | { phase: "loading" }
    | { phase: "error"; message: string }
    | { phase: "ready"; stats: CacheStats }
  >({ phase: "loading" });
  const [clearAgeDays, setClearAgeDays] = useState(DEFAULT_CLEAR_AGE_DAYS);
  const [clearing, setClearing] = useState(false);
  const [confirmingClearAll, setConfirmingClearAll] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const paneRef = useRef<HTMLDivElement>(null);

  const close = useCallback((focusButton: boolean) => {
    setOpen(false);
    setConfirmingClearAll(false);
    if (focusButton) buttonRef.current?.focus();
  }, []);
  const dismiss = useCallback(() => close(false), [close]);
  useOutsideDismiss(menuRef, open, dismiss);

  // Sizes load when the pane opens, and again after a clear. Nothing is
  // fetched while the pane stays closed, and nothing clears on its own.
  // The openers set the loading phase, so the effect only fetches.
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    void (async () => {
      try {
        const stats = await terrainApi.cacheStats(controller.signal);
        if (controller.signal.aborted) return;
        setCache({ phase: "ready", stats });
      } catch (error) {
        if (controller.signal.aborted) return;
        setCache({
          phase: "error",
          message:
            error instanceof TypeError
              ? "Start the local Rust generator to see cache sizes."
              : error instanceof Error
                ? error.message
                : "Cache sizes are unavailable.",
        });
      }
    })();
    return () => controller.abort();
  }, [open, reloadToken]);

  useEffect(() => {
    if (!open) return;
    paneRef.current
      ?.querySelector<HTMLElement>("select:enabled, button:enabled")
      ?.focus();
  }, [open]);

  const paneKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!open || event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    close(true);
  };

  const clear = async (olderThanDays: number | null) => {
    if (clearing) return;
    setClearing(true);
    setConfirmingClearAll(false);
    try {
      const result = await terrainApi.clearCache(olderThanDays);
      setStatus(
        `Removed ${formatBytes(result.removed_bytes)} (${entriesLabel(
          result.removed_entries,
        )}).`,
      );
      setCache({ phase: "loading" });
      setReloadToken((token) => token + 1);
    } catch (error) {
      setStatus(
        error instanceof Error ? error.message : "The cache was not cleared.",
      );
    } finally {
      setClearing(false);
    }
  };

  return (
    <div className="settings-menu" onKeyDown={paneKeyDown} ref={menuRef}>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label="Settings"
        className="settings-button"
        onClick={() => {
          if (open) {
            close(true);
            return;
          }
          setStatus(null);
          setCache({ phase: "loading" });
          setOpen(true);
        }}
        ref={buttonRef}
        type="button"
      >
        <span aria-hidden="true">⚙</span>
      </button>
      {open && (
        <div
          aria-label="Settings"
          className="settings-pane"
          ref={paneRef}
          role="dialog"
        >
          <section aria-label="Map data cache" className="settings-cache">
            <h2>Map data cache</h2>
            {cache.phase === "loading" && (
              <p className="settings-note">Measuring cache…</p>
            )}
            {cache.phase === "error" && (
              <p className="settings-note settings-error" role="alert">
                {cache.message}
              </p>
            )}
            {cache.phase === "ready" && (
              <ul className="settings-cache-rows">
                {cache.stats.categories.map((category) => (
                  <li key={category.key}>
                    <span>
                      {CACHE_CATEGORY_NAMES[category.key] ?? category.key}
                    </span>
                    <span>
                      {formatBytes(category.bytes)}
                      <small> · {entriesLabel(category.entries)}</small>
                    </span>
                  </li>
                ))}
                <li className="settings-cache-total">
                  <span>Total</span>
                  <span>{formatBytes(cache.stats.total_bytes)}</span>
                </li>
              </ul>
            )}
            <div className="settings-clear-row">
              <label>
                Older than
                <select
                  disabled={clearing}
                  onChange={(event) =>
                    setClearAgeDays(Number(event.target.value))
                  }
                  value={clearAgeDays}
                >
                  {CLEAR_AGE_CHOICES.map((days) => (
                    <option key={days} value={days}>
                      {days} days
                    </option>
                  ))}
                </select>
              </label>
              <button
                disabled={clearing}
                onClick={() => {
                  setConfirmingClearAll(false);
                  void clear(clearAgeDays);
                }}
                type="button"
              >
                Clear older
              </button>
              <button
                aria-label={
                  confirmingClearAll
                    ? "Confirm clearing the whole cache"
                    : "Clear all cached map data"
                }
                className={confirmingClearAll ? "confirm-delete" : ""}
                disabled={clearing}
                onClick={() => {
                  if (!confirmingClearAll) {
                    setConfirmingClearAll(true);
                    return;
                  }
                  void clear(null);
                }}
                type="button"
              >
                {confirmingClearAll ? "Confirm" : "Clear all"}
              </button>
            </div>
            <small aria-live="polite" className="settings-status" role="status">
              {status}
            </small>
            <p className="settings-note">
              Clearing is always manual; nothing expires on its own. The next
              generation re-downloads what it needs.
            </p>
          </section>
        </div>
      )}
    </div>
  );
}
