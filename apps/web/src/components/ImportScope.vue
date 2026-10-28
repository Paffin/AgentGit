<script setup lang="ts">
import { migrationScope } from '@/lib/migrationSummary'
</script>

<template>
  <dl class="import-scope">
    <div>
      <dt>Moves with this import</dt>
      <dd>
        <ul>
          <li v-for="item in migrationScope.imported" :key="item">{{ item }}</li>
        </ul>
      </dd>
    </div>
    <div>
      <dt>Move separately</dt>
      <dd>
        <ul>
          <li v-for="item in migrationScope.separate" :key="item">{{ item }}</li>
        </ul>
      </dd>
    </div>
  </dl>
</template>

<style scoped>
.import-scope {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--s-4);
  margin: 0;
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--border);
}
dt {
  margin-bottom: var(--s-1);
  font-weight: 600;
}
dd {
  margin: 0;
  color: var(--text-2);
  line-height: 1.6;
}
ul {
  margin: 0;
  padding-left: 1.1em;
}
li + li {
  margin-top: var(--s-1);
}
@media (max-width: 640px) {
  .import-scope {
    grid-template-columns: 1fr;
    gap: var(--s-3);
  }
}
</style>
