<template>
  <div class="w-full space-y-10 md:max-w-screen-sm lg:max-w-screen-md mx-auto pb-32">
    <Branch
      :json="trees"
      name="Sources:"
    />
    <FooterBar class="mt-20" />
  </div>
</template>

<script>
import FooterBar from './FooterBar.vue';
import Branch from './Branch.vue';

export default {
  components: { Branch, FooterBar },

  props: {
    json: { type: Object, default: () => new Object() }
  },

  computed: {
    trees: function() {
      let trees = {};

      for (let path in this.json) {
        // lookbehind is much cleaner, but it not supported by some browers
        // path.split(/(?<=\/)/)

        path.replace('/', '//').split(/\/(?=[^\/])/).reduce(
          function(acc, dir) {
            if (dir.includes(':')) {
              let [file] = dir.split(':');
              acc[file] = acc[file] || [];
              acc[file].push(this.json[path]);
            } else {
              acc[dir] = acc[dir] || {};
              return acc[dir];
            }
          }.bind(this),
          trees
        );
      }

      return trees;
    }
  }
};
</script>
