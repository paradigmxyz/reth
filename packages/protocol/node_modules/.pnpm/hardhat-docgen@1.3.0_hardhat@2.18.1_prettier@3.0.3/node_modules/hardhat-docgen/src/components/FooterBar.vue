<template>
  <div
    class="bg-gray-100 fixed bottom-0 right-0 w-full border-t border-dashed border-gray-300"
  >
    <div
      class="w-full text-center py-2 md:max-w-screen-sm lg:max-w-screen-md mx-auto"
    >
      <button
        class="py-1 px-2 text-gray-500"
        @click="openLink(repository)"
      >
        built with {{ name }}
      </button>
    </div>
  </div>
</template>

<script>
import { repository, name } from '../../package.json';

export default {
  data: function () {
    return {
      repository,
      name,
    };
  },
  methods: {
    openLink(link) {
      window.open(link, '_blank');
    },
  },
};
</script>
