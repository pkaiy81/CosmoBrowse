#!/usr/bin/env node
import { performance } from 'node:perf_hooks';

function createTabs(count = 20) {
  return Array.from({ length: count }, (_, index) => ({
    id: index + 1,
    title: `Tab ${index + 1}`,
    url: `https://example.test/${index + 1}`,
    is_active: index === count - 1,
    is_pinned: index < 3,
    is_muted: index % 4 === 0,
  }));
}

function renderTabsModel(tabs) {
  return tabs.map((tab, index) => ({
    dataset: { tabId: String(tab.id), tabIndex: String(index) },
    classes: [
      'tab-item',
      tab.is_active ? 'active' : null,
      tab.is_pinned ? 'pinned' : null,
    ].filter(Boolean),
    title: tab.title || tab.url || 'New Tab',
    stateIcons: `${tab.is_pinned ? '📌' : ''}${tab.is_muted ? '🔇' : ''}`,
    actions: ['drag', 'pin', 'mute', 'duplicate', 'close'],
  }));
}

function duplicateTab(tabs, id) {
  const index = tabs.findIndex((tab) => tab.id === id);
  const base = tabs[index];
  const duplicateId = Math.max(...tabs.map((tab) => tab.id)) + 1;
  const duplicate = { ...base, id: duplicateId, is_active: true };
  return [...tabs.slice(0, index + 1), duplicate, ...tabs.slice(index + 1)].map((tab) => ({
    ...tab,
    is_active: tab.id === duplicateId,
  }));
}

function moveTab(tabs, id, targetIndex) {
  const items = [...tabs];
  const sourceIndex = items.findIndex((tab) => tab.id === id);
  const [tab] = items.splice(sourceIndex, 1);
  items.splice(Math.max(0, Math.min(targetIndex, items.length)), 0, tab);
  return items;
}

function togglePinned(tabs, id) {
  return tabs.map((tab) => tab.id === id ? { ...tab, is_pinned: !tab.is_pinned } : tab);
}

function toggleMuted(tabs, id) {
  return tabs.map((tab) => tab.id === id ? { ...tab, is_muted: !tab.is_muted } : tab);
}

function measure(label, fn) {
  const start = performance.now();
  const result = fn();
  const durationMs = performance.now() - start;
  return { label, durationMs, result };
}

let tabs = createTabs(20);
const samples = [];

for (let iteration = 0; iteration < 200; iteration += 1) {
  samples.push(measure('render', () => renderTabsModel(tabs)).durationMs);
  tabs = duplicateTab(tabs, tabs[0].id);
  tabs = moveTab(tabs, tabs[tabs.length - 1].id, 1);
  tabs = togglePinned(tabs, tabs[2].id);
  tabs = toggleMuted(tabs, tabs[3].id);
  tabs = tabs.slice(0, 20);
}

const renderAvg = samples.reduce((sum, value) => sum + value, 0) / samples.length;
const renderP95 = [...samples].sort((a, b) => a - b)[Math.floor(samples.length * 0.95)];
const interaction = measure('interaction_batch', () => {
  let working = createTabs(20);
  working = duplicateTab(working, 4);
  working = moveTab(working, 21, 0);
  working = togglePinned(working, 2);
  working = toggleMuted(working, 3);
  return renderTabsModel(working);
}).durationMs;

const report = {
  tab_count: 20,
  iterations: samples.length,
  render_avg_ms: Number(renderAvg.toFixed(3)),
  render_p95_ms: Number(renderP95.toFixed(3)),
  interaction_batch_ms: Number(interaction.toFixed(3)),
  thresholds: {
    render_p95_ms: 8,
    interaction_batch_ms: 12,
  },
};

console.log(JSON.stringify(report, null, 2));

if (report.render_p95_ms > report.thresholds.render_p95_ms || report.interaction_batch_ms > report.thresholds.interaction_batch_ms) {
  process.exitCode = 1;
}
