"use client";

import type { ComponentPropsWithoutRef, MouseEvent } from "react";

import { IS_TAURI } from "./api";

type ExternalLinkProps = ComponentPropsWithoutRef<"a"> & {
  href: string;
};

export function ExternalLink({
  href,
  onClick,
  rel = "noreferrer",
  target = "_blank",
  ...props
}: ExternalLinkProps) {
  const openInSystemBrowser = async (event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (event.defaultPrevented || !IS_TAURI) return;

    // The click is only taken over once the handler is committed to opening
    // the URL itself, so a failure below has something to report rather than
    // a navigation already cancelled.
    event.preventDefault();
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(href);
    } catch (error) {
      // The desktop build refuses any URL outside the opener scope in
      // src-tauri/capabilities. Swallowing that rejection is what made a
      // missing scope look like a dead link for a whole release, so say so
      // where someone will see it.
      console.error(`Could not open ${href} in the system browser`, error);
      window.alert(
        `TopoSaic could not open this link:\n\n${href}\n\nCopy it into your browser instead.`,
      );
    }
  };

  return (
    <a
      {...props}
      href={href}
      onClick={(event) => void openInSystemBrowser(event)}
      rel={rel}
      target={target}
    />
  );
}
