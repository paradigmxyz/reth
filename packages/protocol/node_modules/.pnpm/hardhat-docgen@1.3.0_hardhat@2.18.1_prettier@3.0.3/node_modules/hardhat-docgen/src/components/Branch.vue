<template>
  <div>
    {{ name }}
    <div v-if="Array.isArray(json)" class="pl-5">
      <div v-for="(contract, index) in json" :key="index">
        <router-link :to="`${contract.source}:${contract.name}`">
          {{ contract.name }}
        </router-link>
      </div>
    </div>
    <div v-else class="pl-5">
      <div v-for="dir of Object.keys(json)" :key="dir">
        <Branch :json="json[dir]" :name="dir" />
      </div>
    </div>
  </div>
</template>

<script>
export default {
  name: 'Branch',

  props: {
    name: { type: String, default: null },
    json: { type: [Object, Array], default: () => new Object() }
  }
};
</script>
