// Feature-wave mock endpoints (docs/ARCHITECTURE.md §12), one module per workstream.
// Created by WS0 and frozen: each owner edits only its own module below.
import type { Show } from '$lib/api/types';
import type { FeatureContext } from './context';
import { featureDemo } from './demo';
import * as autoshow from './autoshow'; // WS2 (F2)
import * as calibration from './calibration'; // WS1 (F1)
import * as journal from './journal'; // WS0 (F11)
import * as library from './library'; // WS2 (F18)
import * as mapping from './mapping'; // WS4 (F6)
import * as pixelcount from './pixelcount'; // WS4 (F7)
import * as power from './power'; // WS3 (F12)
import * as preview from './preview'; // WS2 (F3)
import * as profiles from './profiles'; // WS6 (F8)
import * as remote from './remote'; // WS5 (F14)
import * as reports from './reports'; // WS6 (F11)
import * as sensornodes from './sensornodes'; // WS6 (F20)
import * as tls from './tls'; // WS1 (F1)
import * as triggerlinks from './triggerlinks'; // trigger links (§12.18)
import * as updates from './updates'; // WS5 (F15)
import * as wizard from './wizard'; // WS4 (F9)

export type { FeatureContext };

const MODULES = [
	autoshow,
	calibration,
	journal,
	library,
	mapping,
	pixelcount,
	power,
	preview,
	profiles,
	remote,
	reports,
	sensornodes,
	tls,
	triggerlinks,
	updates,
	wizard
];

export function registerFeatureRoutes(ctx: FeatureContext) {
	for (const m of MODULES) m.register(ctx);
}

/** Fill the demo show with feature-wave data (settings defaults always, examples unless empty). */
export function applyFeatureDemo(show: Show, opts: { empty: boolean }) {
	featureDemo(show, opts);
}
