<template>
  <div class="border-2 border-gray-400 border-dashed w-full p-2">
    <h3 class="text-lg pb-2 mb-2 border-b-2 border-gray-400 border-dashed">
      {{ name }} {{ keywords }} {{ inputSignature }}
    </h3>

    <div class="space-y-3">
      <p>{{ json.notice }}</p>

      <p>{{ json.details }}</p>

      <MemberSection
        name="Parameters"
        :items="inputs"
      />
      <MemberSection
        name="Return Values"
        :items="outputs"
      />
    </div>
  </div>
</template>

<script>
import MemberSection from './MemberSection.vue';
export default {
  components: { MemberSection },
  props: {
    json: { type: Object, default: () => new Object() },
  },

  computed: {
    name: function() {
      // special functions are unnamed; default to type
      return this.json.name || this.json.type;
    },

    keywords: function() {
      let keywords = [];

      if (this.json.stateMutability) {
        keywords.push(this.json.stateMutability);
      }

      if (this.json.anonymous === 'true') {
        keywords.push('anonymous');
      }

      return keywords.join(' ');
    },

    params: function() {
      return this.json.params || {};
    },

    returns: function() {
      return this.json.returns || {};
    },

    inputs: function() {
      return (this.json.inputs || []).map(i => ({
        ...i,
        desc: this.params[i.name],
      }));
    },

    inputSignature: function() {
      return `(${this.inputs.map(i => i.type).join(',')})`;
    },

    outputs: function() {
      return (this.json.outputs || []).map((i, index) => ({
        ...i,
        desc: this.returns[i.name || `_${index}`],
      }));
    },

    outputSignature: function() {
      return `(${this.outputs.map(i => i.type).join(',')})`;
    },
  },
};
</script>
