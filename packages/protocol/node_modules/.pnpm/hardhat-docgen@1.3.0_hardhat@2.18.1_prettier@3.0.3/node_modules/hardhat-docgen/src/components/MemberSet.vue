<template>
  <div class="w-full mt-8">
    <h2 class="text-lg">{{ title }}</h2>
    <Member
      class="mt-3"
      v-for="sig in Object.keys(json)"
      :key="sig"
      :json="json[sig]"
    />
  </div>
</template>

<script>
import Member from './Member.vue';

export default {
  components: { Member },

  props: {
    title: { type: String, default: '' },
    json: { type: Object, default: () => new Object() }
  }
};
</script>
