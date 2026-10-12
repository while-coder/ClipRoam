<script setup lang="ts">
import type { EntryFilter } from "../../types";

const model = defineModel<EntryFilter>({ required: true });
const labels: Record<EntryFilter, string> = { all: "全部", text: "文本", files: "文件", image: "图片" };
const options = Object.values(labels);

function update(value: string[]): void {
  const match = (Object.entries(labels) as [EntryFilter, string][]).find(([, label]) => label === value[0]);
  model.value = match?.[0] ?? "all";
}
</script>

<template>
  <STagFilter class="entry-type-filter" :value="[labels[model]]" :options="options" :multiple="false" @update:value="update" />
</template>
