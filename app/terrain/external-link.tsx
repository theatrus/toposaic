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

    event.preventDefault();
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(href);
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
