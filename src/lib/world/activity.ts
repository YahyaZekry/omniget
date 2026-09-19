// What the activity panel shows: the pose the replica has (from the diffs)
// joined with the reason for it (from the bus, as `world://activity`).
//
// Pure module: no Tauri, no DOM. The panel owns the listeners.

import type { ActivityName } from './codec';

/** Entities from here up are visitors of an open house, never residents. */
export const VISITOR_ENT_BASE = 1000;

/** One resident as the replica sees it. */
export interface AgentRow {
  id: number;
  name: string;
  activity: ActivityName;
  energy: number;
}

/** `AgentWork` of `world_manager.rs`: what the bus said this resident is doing. */
export interface AgentWork {
  agent: string;
  ent: number;
  working: boolean;
  asking: boolean;
  tool: string;
  caption: string;
  conversation: string;
  tools_called: number;
  budget_hit: boolean;
}

export type RowState = 'asking' | 'tool' | 'thinking' | 'tired' | 'sleeping' | 'walking' | 'resting';

/**
 * One word for the row. The bus wins over the pose: an agent walking to its
 * workbench in the middle of a turn is "working", not "walking".
 */
export function rowState(agent: AgentRow, work: AgentWork | undefined): RowState {
  if (work?.asking) return 'asking';
  if (work?.working) return work.tool ? 'tool' : 'thinking';
  if (agent.activity === 'sleeping') return 'sleeping';
  if (work?.budget_hit || agent.energy < 64) return 'tired';
  if (agent.activity === 'walking') return 'walking';
  return 'resting';
}

/** 0..100 from the raw 0..255 energy byte, which is what is left of the quota window. */
export function energyPercent(energy: number): number {
  if (!Number.isFinite(energy)) return 0;
  return Math.round((Math.min(255, Math.max(0, energy)) / 255) * 100);
}
