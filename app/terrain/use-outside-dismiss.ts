"use client";

import { type RefObject, useEffect } from "react";

// Shared popover machinery: while active, a pointerdown outside the
// container dismisses it. The setups menu and the settings pane both hang
// their outside-click handling on this; each keeps its own Escape and
// focus handling because their keyboard models differ (menu rows versus a
// tabbable pane).
export function useOutsideDismiss(
  ref: RefObject<HTMLElement | null>,
  active: boolean,
  onDismiss: () => void,
) {
  useEffect(() => {
    if (!active) return;
    const onPointerDown = (event: PointerEvent) => {
      if (ref.current?.contains(event.target as Node)) return;
      onDismiss();
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [active, onDismiss, ref]);
}
