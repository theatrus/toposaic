import { expect, test } from "@playwright/test";

import { appVersion } from "./helpers";

test("switches between the reflowed control panels", async ({ page }) => {
  await page
    .context()
    .grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");

  const generate = page.getByRole("button", { name: /^Generate/ });
  await expect(page.getByRole("link", { name: "TopoSaic home" })).toContainText(
    "TopoSaic",
  );
  await expect(page.getByRole("link", { name: "TopoSaic home" })).toContainText(
    "Terrain Puzzle",
  );
  await expect(page.getByRole("link", { name: "TopoSaic home" })).toContainText(
    `v${appVersion}`,
  );
  const brandIcon = page.locator(".brand-mark");
  await expect(brandIcon).toHaveCSS("background-image", /url\(.+\)/);
  const brandIconUrl = await brandIcon.evaluate((element) => {
    const background = getComputedStyle(element).backgroundImage;
    return background.match(/^url\(["']?(.*?)["']?\)$/)?.[1] ?? "";
  });
  expect(brandIconUrl).toBeTruthy();
  const brandIconResponse = await page.request.get(
    new URL(brandIconUrl, page.url()).href,
  );
  expect(brandIconResponse.ok()).toBe(true);
  await expect(
    page.getByRole("heading", { name: "Shape your terrain" }),
  ).toBeVisible();
  await expect(page.getByRole("tab")).toHaveText([
    "Model",
    "Surface",
    "Buildings",
    "Markers",
    "Colors",
    "Mounting",
    "Output",
  ]);
  await expect(generate).toHaveAttribute("form", "terrain-controls");
  await expect(page.getByLabel("Find a place")).toBeVisible();
  const elevationSource = page.getByLabel("Elevation tiles");
  await expect(elevationSource).toHaveValue("mapzen");
  await elevationSource.selectOption("mapterhorn");
  await expect(elevationSource).toHaveValue("mapterhorn");
  const fineDemDetail = page.getByRole("checkbox", {
    name: /Use finest available DEM detail/,
  });
  await expect(fineDemDetail).toBeVisible();
  await expect(fineDemDetail).toBeDisabled();
  await page.getByRole("slider", { name: "Ground span" }).fill("2");
  await expect(fineDemDetail).toBeEnabled();
  await expect(fineDemDetail).not.toBeChecked();
  await fineDemDetail.check();
  await expect(fineDemDetail).toBeChecked();
  const mapPosition = page.getByRole("group", { name: "Map position" });
  const superTileMode = page.getByRole("group", { name: "Super-tile mode" });
  await expect(mapPosition).toHaveCSS("border-top-style", "solid");
  const mapPositionBounds = await mapPosition.boundingBox();
  const superTileBounds = await superTileMode.boundingBox();
  expect(mapPositionBounds).not.toBeNull();
  expect(superTileBounds).not.toBeNull();
  expect(Math.abs(mapPositionBounds!.y - superTileBounds!.y)).toBeLessThan(2);
  const puzzleIdentity = page.locator("details.puzzle-seed-advanced");
  await expect(puzzleIdentity).not.toHaveAttribute("open", "");
  await expect(page.getByLabel("Puzzle seed")).toBeHidden();
  const puzzleIdentityBounds = await puzzleIdentity.boundingBox();
  expect(puzzleIdentityBounds).not.toBeNull();
  expect(puzzleIdentityBounds!.y).toBeGreaterThan(
    Math.max(
      mapPositionBounds!.y + mapPositionBounds!.height,
      superTileBounds!.y + superTileBounds!.height,
    ),
  );
  const modelType = page.getByRole("group", { name: "Model type" });
  const puzzleModel = modelType.getByRole("button", {
    name: /Jigsaw puzzle/,
  });
  const solidModel = modelType.getByRole("button", {
    name: /Solid terrain/,
  });
  await expect(modelType).toBeVisible();
  const puzzleModelBounds = await puzzleModel.boundingBox();
  const solidModelBounds = await solidModel.boundingBox();
  expect(puzzleModelBounds).not.toBeNull();
  expect(solidModelBounds).not.toBeNull();
  expect(puzzleModelBounds!.height).toBeLessThan(64);
  expect(puzzleModelBounds!.width).toBeLessThan(340);
  expect(Math.abs(puzzleModelBounds!.y - solidModelBounds!.y)).toBeLessThan(2);
  const pieceLayout = page.getByRole("group", { name: "Piece layout" });
  const modelTypeBounds = await modelType.boundingBox();
  const pieceLayoutBounds = await pieceLayout.boundingBox();
  expect(modelTypeBounds).not.toBeNull();
  expect(pieceLayoutBounds).not.toBeNull();
  expect(modelTypeBounds!.y).toBeLessThan(pieceLayoutBounds!.y);
  await expect(page.getByLabel("Place name")).toBeHidden();
  await solidModel.click();
  await expect(page.getByRole("group", { name: "Piece layout" })).toBeHidden();
  await expect(page.getByText(/2048 across · about 0\.98 m ground spacing/)).toBeVisible();
  await puzzleModel.click();
  await expect(page.getByRole("group", { name: "Piece layout" })).toBeVisible();
  // 2048 samples across 10 pieces round up to 205 per piece, so the
  // assembled model carries 2050 — the same figure the backend reports.
  await expect(page.getByText(/2050 across · about 0\.98 m ground spacing/)).toBeVisible();
  const pieceShape = page.getByRole("group", { name: "Piece shape" });
  const preview = page.getByLabel("Interactive 3D terrain preview");
  const straightGrid = pieceShape.getByRole("checkbox", {
    name: /Straight piece sides/,
  });
  const interlockingTabs = pieceShape.getByRole("checkbox", {
    name: /Interlocking tabs/,
  });
  await expect(straightGrid).not.toBeChecked();
  await expect(interlockingTabs).toBeChecked();
  await straightGrid.check();
  await interlockingTabs.uncheck();
  await expect(preview).toHaveAttribute("data-straight-piece-sides", "true");
  await expect(preview).toHaveAttribute("data-puzzle-tabs", "false");
  await expect(page.getByText("Separate pieces with plain cuts")).toBeVisible();
  const relief = page.getByRole("slider", { name: "Terrain height" });
  await expect(relief).toHaveAttribute("max", "80");
  const initialHeightScale = Number(
    await page
      .getByLabel("Interactive 3D terrain preview")
      .getAttribute("data-height-scale"),
  );
  const initialBaseScale = Number(
    await page
      .getByLabel("Interactive 3D terrain preview")
      .getAttribute("data-base-scale"),
  );
  expect(initialHeightScale).toBeCloseTo(28 / 180, 4);
  expect(initialBaseScale).toBeCloseTo(3.2 / 180, 4);
  await relief.fill("80");
  await expect(relief).toHaveValue("80");
  await expect
    .poll(async () =>
      Number(
        await page
          .getByLabel("Interactive 3D terrain preview")
          .getAttribute("data-height-scale"),
      ),
    )
    .toBeCloseTo(80 / 180, 4);
  const printWidth = page.getByRole("slider", { name: "Print width" });
  await printWidth.fill("300");
  await expect
    .poll(async () =>
      Number(
        await page
          .getByLabel("Interactive 3D terrain preview")
          .getAttribute("data-height-scale"),
      ),
    )
    .toBeCloseTo(80 / 300, 4);
  await expect
    .poll(async () =>
      Number(
        await page
          .getByLabel("Interactive 3D terrain preview")
          .getAttribute("data-base-scale"),
      ),
    )
    .toBeCloseTo(3.2 / 300, 4);

  await page.getByRole("tab", { name: "Surface" }).click();
  const surfaceColors = page.getByRole("group", { name: "Surface settings" });
  await expect(surfaceColors).toBeVisible();
  await expect(
    surfaceColors.getByRole("group", { name: "Terrain appearance" }),
  ).toBeVisible();
  await expect(
    surfaceColors.getByRole("group", { name: "Water" }),
  ).toBeVisible();
  await expect(
    surfaceColors.getByRole("group", { name: "Roads and bridges" }),
  ).toBeVisible();
  await expect(
    surfaceColors.getByRole("group", { name: "Railways and lifts" }),
  ).toBeVisible();
  await expect(
    surfaceColors.getByRole("group", { name: "Imported trails" }),
  ).toBeVisible();
  await expect(page.getByLabel("Find a place")).toBeHidden();
  const floatingBridge = surfaceColors.getByRole("radio", {
    name: "Floating",
  });
  const supportedBridge = surfaceColors.getByRole("radio", {
    name: "Fully supported",
  });
  const bridgeThickness = surfaceColors.getByRole("slider", {
    name: "Floating bridge thickness",
  });
  const routeDetail = surfaceColors.getByLabel("Route detail");
  await expect(routeDetail).toHaveValue("automatic");
  await expect(
    surfaceColors.getByText(
      "At 2 km, automatic mode includes all streets, paths, and trails.",
    ),
  ).toBeVisible();
  const closeViewScaling = surfaceColors.getByRole("checkbox", {
    name: "Boost lines without mapped widths at close views",
  });
  const closeViewBoost = surfaceColors.getByRole("slider", {
    name: "Close-view line boost",
  });
  const maximumMappedWidth = surfaceColors.getByRole("slider", {
    name: "Maximum mapped width",
  });
  const routeMinimumWidth = surfaceColors.getByRole("slider", {
    name: "Route minimum width",
  });
  await expect(closeViewScaling).toBeChecked();
  await expect(closeViewBoost).toHaveValue("2");
  await expect(maximumMappedWidth).toHaveValue("4");
  await expect(
    surfaceColors.getByText(/use 2.00× at 2 km/),
  ).toBeVisible();
  await closeViewBoost.fill("2.5");
  await expect(closeViewBoost).toHaveValue("2.5");
  await maximumMappedWidth.fill("5.5");
  await expect(maximumMappedWidth).toHaveValue("5.5");
  await routeMinimumWidth.fill("4");
  await expect(maximumMappedWidth).toHaveValue("5.6");
  await closeViewScaling.uncheck();
  await expect(closeViewBoost).toBeHidden();
  await closeViewScaling.check();
  await routeDetail.selectOption("streets");
  await expect(routeDetail).toHaveValue("streets");
  await expect(
    surfaceColors.getByText("The chosen detail applies at every map span."),
  ).toBeVisible();
  const classBorders = surfaceColors.getByLabel("Terrain class borders");
  const bendRange = surfaceColors.getByRole("slider", {
    name: "Border bend range",
  });
  const noiseDamping = surfaceColors.getByRole("slider", {
    name: "Border noise damping",
  });
  // Smoothing is the default; the scale gate decides where it engages.
  await expect(classBorders).toHaveValue("smooth");
  await expect(
    surfaceColors.getByText(/Smoothing bends forest, rock, and water borders/),
  ).toBeVisible();
  await expect(bendRange).toHaveValue("2.5");
  await expect(noiseDamping).toHaveValue("0.05");
  await bendRange.fill("4");
  await expect(bendRange).toHaveValue("4");
  await classBorders.selectOption("blocky");
  await expect(classBorders).toHaveValue("blocky");
  await expect(bendRange).toBeHidden();
  await expect(noiseDamping).toBeHidden();
  await classBorders.selectOption("smooth");
  await expect(bendRange).toBeVisible();
  await expect(bendRange).toHaveValue("4");
  const forestSlopeGate = surfaceColors.getByRole("checkbox", {
    name: "Keep forest off steep rock",
  });
  const forestSlopeLimit = surfaceColors.getByRole("slider", {
    name: "Forest slope limit",
  });
  const steepForestTarget = surfaceColors.getByLabel("Steep forest becomes");
  await expect(forestSlopeGate).toBeChecked();
  await expect(forestSlopeLimit).toHaveValue("55");
  await forestSlopeLimit.fill("70");
  await expect(forestSlopeLimit).toHaveValue("70");
  await expect(steepForestTarget).toHaveValue("rock");
  await steepForestTarget.selectOption("snow");
  await expect(steepForestTarget).toHaveValue("snow");
  await forestSlopeGate.uncheck();
  await expect(forestSlopeGate).not.toBeChecked();
  await expect(forestSlopeLimit).toBeHidden();
  await expect(steepForestTarget).toBeHidden();
  await forestSlopeGate.check();
  await expect(forestSlopeLimit).toBeVisible();
  await expect(steepForestTarget).toHaveValue("snow");
  const snowSlopeGate = surfaceColors.getByRole("checkbox", {
    name: "Keep snow off sheer faces",
  });
  const snowSlopeLimit = surfaceColors.getByRole("slider", {
    name: "Snow slope limit",
  });
  await expect(snowSlopeGate).toBeChecked();
  await expect(snowSlopeLimit).toHaveValue("65");
  await snowSlopeLimit.fill("75");
  await expect(snowSlopeLimit).toHaveValue("75");
  await snowSlopeGate.uncheck();
  await expect(snowSlopeGate).not.toBeChecked();
  await expect(snowSlopeLimit).toBeHidden();
  // The two gates are independent: the forest controls stay put.
  await expect(forestSlopeLimit).toBeVisible();
  await snowSlopeGate.check();
  await expect(snowSlopeLimit).toBeVisible();
  await expect(snowSlopeLimit).toHaveValue("75");
  await forestSlopeGate.uncheck();
  await expect(forestSlopeLimit).toBeHidden();
  await expect(snowSlopeLimit).toBeVisible();
  await forestSlopeGate.check();
  await expect(floatingBridge).toBeChecked();
  await expect(bridgeThickness).toHaveValue("1.2");
  await supportedBridge.check();
  await expect(supportedBridge).toBeChecked();
  await expect(bridgeThickness).toBeHidden();
  await floatingBridge.check();
  await bridgeThickness.fill("2.4");
  await expect(bridgeThickness).toHaveValue("2.4");
  await surfaceColors.getByRole("checkbox").first().uncheck();

  await page.getByRole("tab", { name: "Colors" }).click();
  const printColors = page.getByRole("group", { name: "Print colors" });
  await expect(printColors).toBeVisible();
  for (const label of [
    "Forest",
    "Rock",
    "Snow",
    "Water",
    "Route",
    "Trail",
    "Building",
    "Imported trail",
    "Railway",
    "Aerial lift",
    "Map marker",
    "Display base",
    "Base contours",
    "Base label",
  ]) {
    await expect(
      printColors.getByRole("textbox", {
        name: `${label} color`,
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      printColors.getByRole("button", {
        name: `Copy ${label} color`,
        exact: true,
      }),
    ).toBeVisible();
  }
  await expect(
    printColors.getByRole("textbox", { name: "Forest color" }),
  ).toHaveValue("#28543A");
  await expect(
    printColors.getByRole("textbox", { name: "Trail color", exact: true }),
  ).toHaveValue("#D8A33C");
  const routeColor = printColors.getByRole("textbox", {
    name: "Route color",
  });
  const buildingColor = printColors.getByRole("textbox", {
    name: "Building color",
  });
  await routeColor.fill("#8A5B3D");
  const copyRoute = printColors.getByRole("button", {
    name: "Copy Route color",
  });
  await copyRoute.click();
  await expect(copyRoute).toHaveText("Copied");
  const copiedColor = await page.evaluate(() => navigator.clipboard.readText());
  expect(copiedColor).toBe("#8A5B3D");
  await buildingColor.fill(copiedColor);
  await expect(buildingColor).toHaveValue("#8A5B3D");
  const copyPalette = printColors.getByRole("button", {
    name: "Copy all colors",
  });
  await copyPalette.click();
  await expect(copyPalette).toHaveText("Palette copied");
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toContain("Building: #8A5B3D");

  await page.getByRole("tab", { name: "Buildings" }).click();
  const buildings = page.getByRole("group", { name: "Mapped buildings" });
  await expect(buildings).toBeVisible();
  const enableBuildings = buildings.getByRole("checkbox", {
    name: "Enable mapped buildings",
  });
  await expect(enableBuildings).toBeVisible();
  await expect(enableBuildings).not.toBeChecked();
  await enableBuildings.check();
  await expect(
    page
      .getByLabel("Surface color legend")
      .getByText("Building", { exact: true }),
  ).toBeVisible();

  await page.getByRole("tab", { name: "Mounting" }).click();
  const mountingControls = page.getByRole("group", {
    name: "Mounting and display base",
  });
  await expect(mountingControls).toBeVisible();
  const placeName = page.getByLabel("Place name");
  await expect(placeName).toHaveValue("Mount Rainier");
  const fullUnicodeName = `${"山".repeat(47)}𠮷`;
  await placeName.fill(`${fullUnicodeName}余`);
  await expect(placeName).toHaveValue(fullUnicodeName);
  await placeName.fill("富士山 Mount Fuji");
  await expect(placeName).toHaveValue("富士山 Mount Fuji");
  const labelFont = page.getByLabel("Label font");
  await expect(labelFont).toHaveValue("atkinson_hyperlegible");
  await expect(labelFont.locator("option")).toHaveText([
    "Atkinson Hyperlegible",
    "Noto Sans",
    "B612 Mono",
  ]);
  await labelFont.selectOption("b612_mono");
  await expect(labelFont).toHaveValue("b612_mono");
  const labelHeight = page.getByRole("slider", { name: "Label height" });
  await expect(labelHeight).toHaveValue("4");
  await labelHeight.fill("5.5");
  await expect(labelHeight).toHaveValue("5.5");
  const labelPosition = page.getByLabel("Label position");
  await expect(labelPosition).toHaveValue("center");
  await labelPosition.selectOption("right");
  await expect(labelPosition).toHaveValue("right");
  const retention = page.getByRole("checkbox", {
    name: "Pin puzzle into tray",
  });
  await expect(retention).not.toBeChecked();
  await retention.check();
  await expect(page.getByText("Retention pin diameter")).toBeVisible();
  await expect(page.getByText("Retention pin height")).toBeVisible();
  const trayContours = page.getByRole("checkbox", {
    name: "Draw contour lines on tray",
  });
  await expect(trayContours).toBeChecked();
  await trayContours.uncheck();
  await expect(page.getByText("Contour line count")).toBeHidden();
  const floorThickness = page.getByRole("slider", {
    name: "Floor thickness",
  });
  await expect(floorThickness).toHaveAttribute("max", "20");
  const wallMountStyle = page.getByLabel("Wall mount style");
  await expect(wallMountStyle.locator("option")).toHaveText([
    "None",
    "French cleat receiver (recommended)",
    "Angled pin socket",
    "Straight pin socket",
  ]);
  await expect(
    mountingControls.getByText(
      "French cleats spread the load across the full tile or display base",
      { exact: false },
    ),
  ).toBeVisible();
  await wallMountStyle.selectOption("angled_pin");
  await expect(page.getByLabel("Wall mount target")).toHaveValue("tray");
  await expect(page.getByText("Pin engagement depth")).toBeVisible();
  await expect(
    page.getByRole("slider", { name: "Pin engagement depth" }),
  ).toHaveValue("1.6");
  await expect(
    page.getByRole("slider", { name: "Wall plate thickness" }),
  ).toBeVisible();
  await expect(
    page.getByRole("slider", { name: "Pin diameter", exact: true }),
  ).toBeVisible();
  const exportWallHardware = page.getByRole("checkbox", {
    name: "Export matching wall hardware",
  });
  await expect(exportWallHardware).toBeChecked();
  await expect(
    page.getByRole("slider", { name: "Wall offset" }),
  ).toBeVisible();
  await expect(
    page.getByRole("slider", { name: "Mount position from top" }),
  ).toHaveValue("28");
  await expect(
    page.getByRole("slider", { name: "Screw countersink depth" }),
  ).toHaveValue("0.8");
  await expect(
    page.getByRole("slider", { name: "Screw-head pocket clearance" }),
  ).toHaveValue("0.4");
  await expect(
    page.getByRole("slider", { name: "Pocket and hardware fit clearance" }),
  ).toBeVisible();
  await expect(
    page.getByRole("checkbox", { name: "Add edge screw holes on wide mounts" }),
  ).toBeChecked();
  await exportWallHardware.uncheck();
  await expect(
    page.getByRole("slider", { name: "Screw-head pocket clearance" }),
  ).toBeVisible();
  await exportWallHardware.check();
  await wallMountStyle.selectOption("french_cleat");
  await expect(page.getByText("Cleat engagement depth")).toBeVisible();
  await expect(
    page.getByRole("slider", { name: "Cleat engagement depth" }),
  ).toHaveValue("1.6");
  await expect(page.getByText("Cleat slot height")).toBeVisible();
  await expect(page.getByText("Cleat width")).toBeVisible();
  await floorThickness.press("Home");
  await expect(
    page.getByText("display-base floor is too thin", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByText("alignment jig uses the same holes", { exact: false }),
  ).toBeVisible();
  await expect(page.getByText("flat alignment spacer", { exact: false })).toBeVisible();
  await mountingControls.getByRole("checkbox").first().uncheck();
  await expect(retention).not.toBeChecked();
  await expect(page.getByLabel("Wall mount target")).toHaveValue("terrain");

  await page.getByRole("tab", { name: "Output" }).click();
  await expect(page.getByText("No generation job yet.")).toBeVisible();
  const threeMfStyle = page.getByLabel("3MF style");
  await expect(threeMfStyle).toHaveValue("project");
  await expect(
    page.getByText(/OrcaSlicer and Bambu\s+Studio import the file as a project/),
  ).toBeVisible();
  await threeMfStyle.selectOption("painted");
  await expect(threeMfStyle).toHaveValue("painted");
  await threeMfStyle.selectOption("geometry");
  await expect(threeMfStyle).toHaveValue("geometry");
  await threeMfStyle.selectOption("project");
  await expect(threeMfStyle).toHaveValue("project");
  await expect(
    page.getByRole("link", { name: "Mapterhorn elevation tiles" }),
  ).toHaveAttribute("href", "https://mapterhorn.com/attribution");

  await page.getByRole("tab", { name: "Model" }).click();
  await expect(page.getByLabel("Find a place")).toBeVisible();
});

test("switches railways on apart from roads and submits them", async ({
  page,
}) => {
  let jobSpec: Record<string, unknown> = {};
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/jobs" && request.method() === "POST") {
      jobSpec = request.postDataJSON();
      await route.fulfill({
        status: 202,
        json: {
          id: "rail-job",
          status: "running",
          progress: 10,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("tab", { name: "Colors" }).click();
  const printColors = page.getByRole("group", { name: "Print colors" });
  const railColor = printColors.getByRole("textbox", {
    name: "Railway color",
  });
  await expect(railColor).toHaveValue("#C43D3D");
  await railColor.fill("#2B3440");

  await page.getByRole("tab", { name: "Surface" }).click();
  const surfaceColors = page.getByRole("group", { name: "Surface settings" });
  const railways = surfaceColors.getByRole("checkbox", {
    name: "Render railways",
  });
  const railStyle = surfaceColors.getByLabel("Railway style");
  const railWidth = surfaceColors.getByRole("slider", {
    name: "Railway minimum width",
  });
  const railLegend = page
    .getByLabel("Surface color legend")
    .getByText("Rail", { exact: true });

  // The layer starts on in its own color, so its width and legend entry are
  // there from the start.
  await expect(railways).toBeChecked();
  await expect(railStyle).toHaveValue("separate");
  await expect(railWidth).toHaveValue("0.7");
  await expect(railLegend).toBeVisible();
  await expect(
    surfaceColors.getByText(/trains, trams, metros, narrow gauge/),
  ).toBeVisible();
  // The slot is only spent where the mapped data holds railways.
  await expect(
    surfaceColors.getByText(/uses a filament slot only where the map has a/),
  ).toBeVisible();

  // Switching the layer off takes its style, width, and legend entry with it.
  // Its saved color stays in the Colors tab.
  await railways.uncheck();
  await expect(railStyle).toBeHidden();
  await expect(railWidth).toBeHidden();
  await expect(railLegend).toBeHidden();
  await railways.check();
  await expect(railStyle).toHaveValue("separate");

  // Folded into the roads they take the route color, so they need no width of
  // their own and no legend entry.
  await railStyle.selectOption("with_roads");
  await expect(railWidth).toBeHidden();
  await expect(railLegend).toBeHidden();

  await railStyle.selectOption("separate");
  await railWidth.fill("1.2");
  await expect(railWidth).toHaveValue("1.2");

  // Railways switch independently of roads: turning roads off leaves the
  // railway controls in place.
  const roads = surfaceColors.getByRole("checkbox", { name: "Render roads" });
  const routeLegend = page
    .getByLabel("Surface color legend")
    .getByText("Route", { exact: true });
  await roads.uncheck();
  await expect(surfaceColors.getByLabel("Route detail")).toBeHidden();
  await expect(railways).toBeChecked();
  await expect(railStyle).toBeVisible();
  await expect(railWidth).toHaveValue("1.2");
  await expect(railLegend).toBeVisible();
  await expect(routeLegend).toBeHidden();

  // Rails folded into the roads print in the route color, so a rail-only
  // model keeps the route entry that names that color.
  await railStyle.selectOption("with_roads");
  await expect(routeLegend).toBeVisible();
  await expect(railLegend).toBeHidden();
  // Switching the railways off then leaves nothing in the road class —
  // lifts have their own color and do not stand in for it.
  await railways.uncheck();
  await expect(routeLegend).toBeHidden();
  await railways.check();
  await railStyle.selectOption("separate");
  await roads.check();
  await expect(routeLegend).toBeVisible();

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect
    .poll(() => (jobSpec.color_output as Record<string, unknown>)?.rail_enabled)
    .toBe(true);
  const colorOutput = jobSpec.color_output as Record<string, unknown>;
  expect(colorOutput.rail_style).toBe("separate");
  expect(colorOutput.rail_color).toBe("#2B3440");
  expect(colorOutput.rail_width_mm).toBe(1.2);
  expect(colorOutput.scale_line_widths_by_span).toBe(true);
  expect(colorOutput.close_view_width_multiplier).toBe(2);
  expect(colorOutput.maximum_mapped_width_mm).toBe(4);

  // The chosen settings survive a trip away from the tab.
  await page.getByRole("tab", { name: "Surface" }).click();
  await expect(railways).toBeChecked();
  await expect(railStyle).toHaveValue("separate");
  await expect(railWidth).toHaveValue("1.2");
  await page.getByRole("tab", { name: "Colors" }).click();
  await expect(railColor).toHaveValue("#2B3440");
});

test("draws aerial lifts apart from railways and names every color", async ({
  page,
}) => {
  let jobSpec: Record<string, unknown> = {};
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/jobs" && request.method() === "POST") {
      jobSpec = request.postDataJSON();
      await route.fulfill({
        status: 202,
        json: {
          id: "aerial-job",
          status: "running",
          progress: 10,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("tab", { name: "Colors" }).click();
  const printColors = page.getByRole("group", { name: "Print colors" });
  const liftColor = printColors.getByRole("textbox", {
    name: "Aerial lift color",
  });
  await expect(liftColor).toHaveValue("#6C4CB6");
  await liftColor.fill("#7D3FA0");

  await page.getByRole("tab", { name: "Surface" }).click();
  const surfaceColors = page.getByRole("group", { name: "Surface settings" });
  const railways = surfaceColors.getByRole("checkbox", {
    name: "Render railways",
  });
  const railStyle = surfaceColors.getByLabel("Railway style");
  const lifts = surfaceColors.getByRole("checkbox", {
    name: "Render aerial lifts",
  });
  const liftStyle = surfaceColors.getByLabel("Aerial lift style");
  const liftWidth = surfaceColors.getByRole("slider", {
    name: "Aerial lift print width",
  });
  const history = surfaceColors.getByLabel("Railway and lift history");
  const legend = page.getByLabel("Surface color legend");
  const routeLegend = legend.getByText("Route", { exact: true });
  const railLegend = legend.getByText("Rail", { exact: true });
  const liftLegend = legend.getByText("Aerial", { exact: true });

  // Lifts default on in their own color, alongside railways in theirs, so
  // all three line entries name themselves from the start.
  await expect(lifts).toBeChecked();
  await expect(liftStyle).toHaveValue("separate");
  await expect(liftWidth).toHaveValue("0.7");
  await expect(routeLegend).toBeVisible();
  await expect(railLegend).toBeVisible();
  await expect(liftLegend).toBeVisible();
  await expect(
    surfaceColors.getByText(/Funiculars run on the ground/),
  ).toBeVisible();
  await expect(
    surfaceColors.getByText(/slot only where mapped lifts exist/),
  ).toBeVisible();

  // Folded into the railways, lifts follow them wherever they go: into the
  // rail color first, then into the route color with them.
  await liftStyle.selectOption("with_rail");
  await expect(liftWidth).toBeHidden();
  await expect(liftLegend).toBeHidden();
  await expect(railLegend).toBeVisible();
  // Lifts painting in the rail color are named by the Rail entry, so with
  // roads off nothing is left for the route entry to name.
  const roads = surfaceColors.getByRole("checkbox", { name: "Render roads" });
  await roads.uncheck();
  await expect(routeLegend).toBeHidden();
  await railStyle.selectOption("with_roads");
  await expect(railLegend).toBeHidden();
  await expect(routeLegend).toBeVisible();
  await railStyle.selectOption("separate");
  await roads.check();

  // Their own color splits them back out of the rail family.
  await liftStyle.selectOption("separate");
  await expect(liftWidth).toHaveValue("0.7");
  await expect(liftLegend).toBeVisible();
  await expect(railLegend).toBeVisible();
  await liftWidth.fill("0.9");

  // Lifts switch independently of railways: the railway toggle leaves the
  // lift controls and the lift legend entry alone.
  await railways.uncheck();
  await expect(railStyle).toBeHidden();
  await expect(railLegend).toBeHidden();
  await expect(lifts).toBeChecked();
  await expect(liftStyle).toHaveValue("separate");
  await expect(liftLegend).toBeVisible();

  // Following railways with railways switched off falls through to roads
  // rather than leaving an enabled layer drawing nothing.
  await liftStyle.selectOption("with_rail");
  await expect(liftLegend).toBeHidden();
  await expect(railLegend).toBeHidden();
  await expect(routeLegend).toBeVisible();
  await expect(
    surfaceColors.getByText(/Railways are off, so lifts use the road color/),
  ).toBeVisible();
  await railways.check();
  await liftStyle.selectOption("separate");

  // One history setting serves both layers, and stays while either is on.
  await expect(history).toHaveValue("operational");
  await history.selectOption("abandoned");
  await expect(history).toHaveValue("abandoned");
  await railways.uncheck();
  await expect(history).toBeVisible();
  await lifts.uncheck();
  await expect(history).toBeHidden();
  await lifts.check();
  await expect(history).toHaveValue("abandoned");
  await railways.check();

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect
    .poll(
      () => (jobSpec.color_output as Record<string, unknown>)?.aerial_style,
    )
    .toBe("separate");
  const colorOutput = jobSpec.color_output as Record<string, unknown>;
  expect(colorOutput.aerial_enabled).toBe(true);
  expect(colorOutput.aerial_color).toBe("#7D3FA0");
  expect(colorOutput.aerial_width_mm).toBe(0.9);
  expect(colorOutput.rail_lifecycle).toBe("abandoned");

  await page.getByRole("tab", { name: "Surface" }).click();
  await expect(liftStyle).toHaveValue("separate");
  await expect(history).toHaveValue("abandoned");
  await page.getByRole("tab", { name: "Colors" }).click();
  await expect(liftColor).toHaveValue("#7D3FA0");
});

test("uses the selected elevation source for live previews", async ({
  page,
}) => {
  const previewSources: string[] = [];
  await page.route(
    "http://127.0.0.1:8787/api/preview",
    async (route, request) => {
      previewSources.push(request.postDataJSON().elevation_source);
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
    },
  );
  await page.goto("/");

  await expect.poll(() => previewSources).toContain("mapzen");
  await page.getByLabel("Elevation tiles").selectOption("mapterhorn");
  await expect.poll(() => previewSources).toContain("mapterhorn");

  await page.getByRole("tab", { name: "Output" }).click();
  await expect(
    page.getByRole("link", { name: "Mapterhorn elevation tiles" }),
  ).toHaveAttribute("href", "https://mapterhorn.com/attribution");
});

test("resizes the preview area to make room for controls", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");

  const resizer = page.getByRole("separator", {
    name: "Resize map and 3D preview",
  });
  const visualArea = page.locator(".visual-column");
  const controls = page.locator("#terrain-controls");

  await expect(resizer).toBeVisible();
  await expect(resizer).toHaveAttribute("aria-orientation", "horizontal");
  await expect(resizer).toHaveAttribute("aria-valuenow", "37");

  const initialVisualBounds = await visualArea.boundingBox();
  const initialControlBounds = await controls.boundingBox();
  expect(initialVisualBounds).not.toBeNull();
  expect(initialControlBounds).not.toBeNull();
  expect(
    initialControlBounds!.height /
      (initialVisualBounds!.height + initialControlBounds!.height),
  ).toBeCloseTo(0.63, 2);

  await resizer.focus();
  await page.keyboard.press("Home");
  await expect(resizer).toHaveAttribute("aria-valuenow", "28");

  const smallVisualBounds = await visualArea.boundingBox();
  const largeControlBounds = await controls.boundingBox();
  expect(smallVisualBounds).not.toBeNull();
  expect(largeControlBounds).not.toBeNull();
  expect(smallVisualBounds!.height).toBeLessThan(initialVisualBounds!.height);
  expect(largeControlBounds!.height).toBeGreaterThan(
    initialControlBounds!.height,
  );

  const resizerBounds = await resizer.boundingBox();
  expect(resizerBounds).not.toBeNull();
  if (!resizerBounds) return;
  await page.mouse.move(
    resizerBounds.x + resizerBounds.width / 2,
    resizerBounds.y + resizerBounds.height / 2,
  );
  await page.mouse.down();
  await page.mouse.move(
    resizerBounds.x + resizerBounds.width / 2,
    resizerBounds.y + 120,
    { steps: 6 },
  );
  await page.mouse.up();

  await expect
    .poll(async () => Number(await resizer.getAttribute("aria-valuenow")))
    .toBeGreaterThan(28);
});

test("supports linked and map-only zoom controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");

  const groundSpan = page.getByRole("slider", { name: "Ground span" });
  await expect(groundSpan).toHaveAttribute("min", "0.25");
  await expect(groundSpan).toHaveAttribute("step", "0.25");
  const selection = page.locator(".map-selection");
  await expect(selection).toHaveAttribute(
    "aria-label",
    "Selected terrain area: 18 km square",
  );
  const initialBounds = await selection.boundingBox();
  expect(initialBounds).not.toBeNull();
  await expect(selection).toHaveAttribute("data-map-zoom", "9");
  const meshDetail = page.getByRole("radiogroup", { name: "Mesh detail" });
  await expect(
    meshDetail.getByRole("radio", { name: /Standard/ }),
  ).toHaveAttribute("aria-checked", "true");
  await meshDetail.getByRole("radio", { name: /Ultra/ }).click();
  await expect(
    meshDetail.getByRole("radio", { name: /Ultra/ }),
  ).toHaveAttribute("aria-checked", "true");
  // Ultra's 2048 samples round up to 205 per piece across 10 pieces, so
  // the assembled label reads 2050, matching the backend.
  await expect(
    page.getByText(/2050 across · about 8\.8 m ground spacing/),
  ).toBeVisible();
  await meshDetail.getByRole("radio", { name: /Standard/ }).click();
  await expect(
    page.getByText(/640 across · about 28\.1 m ground spacing/),
  ).toBeVisible();

  const zoomMode = page.getByRole("button", {
    name: "Resize selected area with map zoom",
  });
  await expect(zoomMode).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "Zoom in" }).click();
  await expect(groundSpan).toHaveValue("9");
  await expect(selection).toHaveAttribute(
    "aria-label",
    "Selected terrain area: 9 km square",
  );
  await expect(selection).toHaveAttribute("data-map-zoom", "10");
  await expect(
    page.getByText(/640 across · about 14\.1 m ground spacing/),
  ).toBeVisible();

  const linkedZoomBounds = await selection.boundingBox();
  expect(linkedZoomBounds).not.toBeNull();
  expect(linkedZoomBounds!.width).toBeCloseTo(initialBounds!.width, 0);

  await zoomMode.click();
  await expect(zoomMode).toHaveAttribute("aria-pressed", "false");
  await expect(zoomMode).toHaveText("Map only");
  await page.getByRole("button", { name: "Zoom out" }).click();
  await expect(selection).toHaveAttribute("data-map-zoom", "9");
  await expect(groundSpan).toHaveValue("9");
  const mapOnlyZoomBounds = await selection.boundingBox();
  expect(mapOnlyZoomBounds).not.toBeNull();
  expect(mapOnlyZoomBounds!.width).toBeCloseTo(initialBounds!.width / 2, 0);

  const map = page.locator(".map-canvas");
  await map.hover();
  await page.mouse.wheel(0, -100);
  await expect(selection).toHaveAttribute("data-map-zoom", "10");
  await expect(groundSpan).toHaveValue("9");
  const wheelZoomedBounds = await selection.boundingBox();
  expect(wheelZoomedBounds).not.toBeNull();
  expect(wheelZoomedBounds!.width).toBeCloseTo(initialBounds!.width, 0);

  await groundSpan.fill("30");
  await expect(selection).toHaveAttribute(
    "aria-label",
    "Selected terrain area: 30 km square",
  );
  await expect(selection).toHaveAttribute("data-ground-span-km", "30");
  const largerBounds = await selection.boundingBox();
  expect(largerBounds).not.toBeNull();
  expect(largerBounds!.width).toBeGreaterThan(mapOnlyZoomBounds!.width);
});

test("locks a height frame and maps a super-tile grid", async ({
  page,
}) => {
  const previewSpecs: Array<Record<string, unknown>> = [];
  await page.route("http://127.0.0.1:8787/api/preview", async (route) => {
    const spec = route.request().postDataJSON() as Record<string, unknown>;
    previewSpecs.push(spec);
    const moved = Number(spec.center_lon) > -121.7;
    const minimum = moved ? 80 : 100;
    const datum = spec.elevation_datum_m;
    await route.fulfill({
      json: {
        width: 2,
        height: 2,
        values: [0, 0.3, 0.7, 1],
        minimum_elevation_m: minimum,
        maximum_elevation_m: moved ? 280 : 300,
        height_frame_compatible:
          datum === null || datum === undefined || minimum >= Number(datum),
      },
    });
  });

  await page.goto("/");
  await expect(page.getByText("Live elevation preview")).toBeVisible();
  await page.getByText("Advanced puzzle identity", { exact: true }).click();
  const puzzleSeed = page.getByLabel("Puzzle seed");
  const reseed = page.getByRole("button", { name: "Generate new seed" });
  await expect(reseed).toBeVisible();
  await expect(reseed).toHaveCSS("border-top-style", "solid");
  await expect.poll(async () => Number(await puzzleSeed.inputValue())).not.toBe(0);
  await puzzleSeed.fill("305419896");
  await expect(puzzleSeed).toHaveValue("305419896");
  await expect(page.getByLabel("Tile column")).toHaveValue("0");
  await expect(page.getByLabel("Tile row")).toHaveValue("0");

  const minimumHeight = page.getByRole("slider", {
    name: "Minimum piece height",
  });
  await expect(minimumHeight).toHaveAttribute("max", "20");
  await expect(minimumHeight).toHaveValue("3.2");
  await minimumHeight.fill("5");
  await expect(minimumHeight).toHaveValue("5");

  const initialLongitude = Number(
    await page.getByLabel("Longitude").inputValue(),
  );
  await page
    .getByRole("group", { name: "Super-tile mode" })
    .getByRole("button", { name: /east/i })
    .click();

  await expect(page.getByText(/Moved east by one tile/)).toBeVisible();
  await expect(page.getByLabel("Tile column")).toHaveValue("1");
  await expect(page.getByLabel("Tile row")).toHaveValue("0");
  await expect(page.getByText(/Shared datum 96\.0 m/)).toBeVisible();
  await expect
    .poll(async () => Number(await page.getByLabel("Longitude").inputValue()))
    .toBeGreaterThan(initialLongitude);
  await expect(
    page.getByRole("alert").filter({ hasText: "drops below the shared" }),
  ).toBeVisible();
  expect(
    previewSpecs.some(
      (spec) =>
        spec.elevation_datum_m === 96 &&
        Number(spec.elevation_m_per_mm) > 0,
    ),
  ).toBe(true);

  await page.getByRole("button", { name: "Unlock height" }).click();
  await expect(page.getByText(/manual neighbors may form a step/)).toBeVisible();

  const superTileControls = page.getByLabel("Super-tile grid");
  const latitudeBounds = await page.getByLabel("Latitude").boundingBox();
  const adjacentBounds = await page
    .getByRole("group", { name: "Super-tile mode" })
    .boundingBox();
  expect(latitudeBounds).not.toBeNull();
  expect(adjacentBounds).not.toBeNull();
  expect(adjacentBounds!.x).toBeGreaterThan(latitudeBounds!.x);
  expect(adjacentBounds!.y).toBeLessThan(
    latitudeBounds!.y + latitudeBounds!.height,
  );
  await superTileControls.getByLabel("Across").selectOption("8");
  await superTileControls.getByLabel("Down").selectOption("6");
  await expect(page.getByText(/48 terrain 3MF files/)).toBeVisible();
  const mapGrid = page.getByRole("group", {
    name: "Super-tile map: 8 across by 6 down, anchored at top-left tile",
  });
  await expect(mapGrid).toHaveAttribute("data-super-tile-columns", "8");
  await expect(mapGrid).toHaveAttribute("data-super-tile-rows", "6");
  await expect(page.locator(".map-selection")).toHaveCount(48);
  await expect(
    page.getByText(/Super-tile mode · 8 × 6 · current tile is top-left tile/),
  ).toBeVisible();
  const currentMapTile = page.locator(
    '.map-selection[data-super-tile-row="1"][data-super-tile-column="1"]',
  );
  const eastMapTile = page.locator(
    '.map-selection[data-super-tile-row="1"][data-super-tile-column="2"]',
  );
  const southMapTile = page.locator(
    '.map-selection[data-super-tile-row="2"][data-super-tile-column="1"]',
  );
  const currentMapBounds = await currentMapTile.boundingBox();
  const eastMapBounds = await eastMapTile.boundingBox();
  const southMapBounds = await southMapTile.boundingBox();
  expect(currentMapBounds).not.toBeNull();
  expect(eastMapBounds).not.toBeNull();
  expect(southMapBounds).not.toBeNull();
  expect(eastMapBounds!.x).toBeGreaterThan(currentMapBounds!.x);
  expect(southMapBounds!.y).toBeGreaterThan(currentMapBounds!.y);
  const anchorChoice = page.getByRole("radiogroup", {
    name: "Super-tile anchor",
  });
  await expect(
    anchorChoice.getByRole("radio", { name: "Top-left tile" }),
  ).toBeChecked();
  await anchorChoice.getByRole("radio", { name: "Center tile" }).check();
  await expect(
    page.getByText(/grid changed to 9 × 7/),
  ).toBeVisible();
  await expect(superTileControls.getByLabel("Across")).toHaveValue("9");
  await expect(superTileControls.getByLabel("Down")).toHaveValue("7");
  await expect(page.locator(".map-selection")).toHaveCount(63);
  const centeredMapGrid = page.getByRole("group", {
    name: "Super-tile map: 9 across by 7 down, anchored at center tile",
  });
  await expect(centeredMapGrid).toHaveAttribute(
    "data-super-tile-columns",
    "9",
  );
  await expect(centeredMapGrid).toHaveAttribute("data-super-tile-rows", "7");
  const centeredMapTile = page.locator(
    '.map-selection.current[data-super-tile-row="4"][data-super-tile-column="5"]',
  );
  await expect(centeredMapTile).toHaveCount(1);
  const tileInterlocks = page.getByRole("checkbox", {
    name: /Interlock super-tile and tray joins/,
  });
  await tileInterlocks.check();
  await expect(tileInterlocks).toBeChecked();
  const outerEdgeInterlocks = page.getByRole("checkbox", {
    name: /Add notches to outer super-tile edges/,
  });
  await expect(outerEdgeInterlocks).not.toBeChecked();
  await outerEdgeInterlocks.check();
  await expect(outerEdgeInterlocks).toBeChecked();

  await page.getByRole("tab", { name: "Mounting" }).click();
  const separateTrays = page.getByRole("checkbox", {
    name: /Separate framed trays/,
  });
  await expect(separateTrays).toBeVisible();
  await separateTrays.check();
  await expect(separateTrays).toBeChecked();
  const wallMountStyle = page.getByLabel("Wall mount style");
  await wallMountStyle.selectOption("straight_pin");
  await expect(
    page.getByText(/Print 63 copies of the wall-side hardware/),
  ).toBeVisible();
  await page.getByLabel("Wall mount target").selectOption("tray");
  await expect(
    page.getByText(/Print 63 copies of the wall-side hardware/),
  ).toBeVisible();
});

test("rotates, zooms, and resets the interactive 3D preview", async ({
  page,
}) => {
  await page.goto("/");

  const preview = page.getByLabel("Interactive 3D terrain preview");
  await expect(preview).toBeVisible();
  await expect(
    page.getByText("Drag to rotate · Scroll or pinch to zoom"),
  ).toBeVisible();
  await expect(preview).toHaveAttribute("data-camera-moved", "false");

  const bounds = await preview.boundingBox();
  expect(bounds).not.toBeNull();
  if (!bounds) return;
  await page.mouse.move(
    bounds.x + bounds.width * 0.68,
    bounds.y + bounds.height * 0.62,
  );
  await page.mouse.down();
  await page.mouse.move(
    bounds.x + bounds.width * 0.42,
    bounds.y + bounds.height * 0.4,
    { steps: 8 },
  );
  await page.mouse.up();
  await expect(preview).toHaveAttribute("data-camera-moved", "true");

  await page.getByRole("button", { name: "Reset view" }).click();
  await expect(preview).toHaveAttribute("data-camera-moved", "false");

  await preview.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(preview).toHaveAttribute("data-camera-moved", "true");
});

test("keeps the map and preview split adjustable from the keyboard", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");

  const resizer = page.getByRole("separator", {
    name: "Resize map and preview panes",
  });
  await expect(resizer).toBeVisible();
  await expect(resizer).toHaveAttribute("aria-orientation", "vertical");
  await expect(resizer).toHaveAttribute("aria-valuenow", "50");

  const mapShell = page.locator(".map-shell");
  const initialBounds = await mapShell.boundingBox();
  expect(initialBounds).not.toBeNull();

  await resizer.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(resizer).toHaveAttribute("aria-valuenow", "46");
  await page.keyboard.press("ArrowRight");
  await expect(resizer).toHaveAttribute("aria-valuenow", "50");
  await page.keyboard.press("Home");
  await expect(resizer).toHaveAttribute("aria-valuenow", "25");

  const narrowBounds = await mapShell.boundingBox();
  expect(narrowBounds).not.toBeNull();
  expect(narrowBounds!.width).toBeLessThan(initialBounds!.width);

  await page.keyboard.press("End");
  await expect(resizer).toHaveAttribute("aria-valuenow", "75");
  await resizer.dblclick();
  await expect(resizer).toHaveAttribute("aria-valuenow", "50");
});
