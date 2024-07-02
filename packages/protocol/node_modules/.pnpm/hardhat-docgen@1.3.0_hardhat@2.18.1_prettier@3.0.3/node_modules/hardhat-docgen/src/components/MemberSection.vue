<template>
  <ul v-if="items.length > 0">
    <h4 class="text-lg">
      {{ name }}
    </h4>
    <li
      v-for="(item, index) in items"
      :key="index"
    >
      <span class="bg-gray-300">{{ item.type }}</span>
      <b>{{ item.name || `_${index}` }}</b><span v-if="item.desc">: <i>{{ item.desc }}</i></span>
    </li>
  </ul>
</template>

<script>
export default {
  props: {
    name: { type: String, default: '' },
    items: { type: Array, default: () => new Array() },
  },
};
</script>
