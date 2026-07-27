import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import type {
  UpdateGenerationSpec,
  UpdatePuzzleRetention,
  UpdateTray,
  UpdateWallMount,
} from "./mounting-types";
import { DisplayBaseControls } from "./display-base-controls";
import { PuzzleRetentionControls } from "./puzzle-retention-controls";
import { WallMountControls } from "./wall-mount-controls";

export function MountingPanel({
  hidden,
  spec,
  update,
  updateTray,
  updatePuzzleRetention,
  updateWallMount,
}: {
  hidden: boolean;
  spec: GenerationSpec;
  update: UpdateGenerationSpec;
  updateTray: UpdateTray;
  updatePuzzleRetention: UpdatePuzzleRetention;
  updateWallMount: UpdateWallMount;
}) {
  const mountEnabled = spec.wall_mount.style !== "none";

  useEffect(() => {
    if (
      spec.puzzle_retention.enabled &&
      mountEnabled &&
      spec.wall_mount.target === "terrain"
    ) {
      updateWallMount("target", "tray");
    }
  }, [
    mountEnabled,
    spec.puzzle_retention.enabled,
    spec.wall_mount.target,
    updateWallMount,
  ]);

  const setTrayEnabled = (enabled: boolean) => {
    updateTray("enabled", enabled);
    if (!enabled) {
      updatePuzzleRetention("enabled", false);
      if (spec.wall_mount.target === "tray") {
        updateWallMount("target", "terrain");
      }
    }
  };

  return (
    <fieldset
      className="color-controls mounting-controls control-section"
      aria-label="Mounting and display base"
      hidden={hidden}
    >
      <DisplayBaseControls
        spec={spec}
        update={update}
        updateTray={updateTray}
        setTrayEnabled={setTrayEnabled}
      />
      <PuzzleRetentionControls
        spec={spec}
        updatePuzzleRetention={updatePuzzleRetention}
      />
      <WallMountControls spec={spec} updateWallMount={updateWallMount} />
    </fieldset>
  );
}
