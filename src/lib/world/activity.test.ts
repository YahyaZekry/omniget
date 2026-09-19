import { describe, expect, it } from 'vitest';
import { energyPercent, rowState, type AgentRow, type AgentWork } from './activity';

const agent = (over: Partial<AgentRow> = {}): AgentRow => ({
  id: 1,
  name: 'builder',
  activity: 'idle',
  energy: 255,
  ...over,
});

const work = (over: Partial<AgentWork> = {}): AgentWork => ({
  agent: 'builder',
  ent: 1,
  working: false,
  asking: false,
  tool: '',
  caption: '',
  conversation: '',
  tools_called: 0,
  budget_hit: false,
  ...over,
});

describe('rowState', () => {
  it('lets the bus win over the pose', () => {
    expect(rowState(agent({ activity: 'walking' }), work({ working: true }))).toBe('thinking');
    expect(rowState(agent({ activity: 'walking' }), work({ working: true, tool: 'fs_edit' }))).toBe('tool');
    expect(rowState(agent({ activity: 'working' }), work({ working: true, asking: true }))).toBe('asking');
  });

  it('falls back to the pose when no turn is running', () => {
    expect(rowState(agent({ activity: 'walking' }), undefined)).toBe('walking');
    expect(rowState(agent({ activity: 'sleeping', energy: 0 }), undefined)).toBe('sleeping');
    expect(rowState(agent({ energy: 40 }), work({ budget_hit: true }))).toBe('tired');
    expect(rowState(agent(), work())).toBe('resting');
  });
});

describe('energyPercent', () => {
  it('maps the energy byte onto a percentage and never leaves 0..100', () => {
    expect(energyPercent(255)).toBe(100);
    expect(energyPercent(0)).toBe(0);
    expect(energyPercent(128)).toBe(50);
    expect(energyPercent(900)).toBe(100);
    expect(energyPercent(Number.NaN)).toBe(0);
  });
});
