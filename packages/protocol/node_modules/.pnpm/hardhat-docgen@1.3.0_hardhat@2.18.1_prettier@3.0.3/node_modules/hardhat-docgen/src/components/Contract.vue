<template>
  <div class="w-full space-y-10 md:max-w-screen-sm lg:max-w-screen-md mx-auto">
    <HeaderBar />
    <div class="pb-32">
      <div class="space-y-4">
        <span class="text-lg">
          {{ json.source }}
        </span>

        <h1 class="text-xl">
          {{ json.name }}
        </h1>

        <h2 class="text-lg">
          {{ json.title }}
        </h2>

        <h2 class="text-lg">
          {{ json.author }}
        </h2>

        <p>{{ json.notice }}</p>

        <p>{{ json.details }}</p>
      </div>

      <div class="mt-8">
        <Member
          v-if="json.hasOwnProperty('constructor')"
          :json="json.constructor"
        />
      </div>
      <div class="mt-8">
        <Member
          v-if="json.receive"
          :json="json.receive"
        />
      </div>
      <div class="mt-8">
        <Member
          v-if="json.fallback"
          :json="json.fallback"
        />
      </div>

      <MemberSet
        v-if="json.events"
        title="Events"
        :json="json.events"
      />

      <MemberSet
        v-if="json.stateVariables"
        title="State Variables"
        :json="json.stateVariables"
      />

      <MemberSet
        v-if="json.methods"
        title="Methods"
        :json="json.methods"
      />
    </div>
    <FooterBar />
  </div>
</template>

<script>
import FooterBar from './FooterBar.vue';
import HeaderBar from './HeaderBar.vue';
import Member from './Member.vue';
import MemberSet from './MemberSet.vue';

export default {
  components: { Member, MemberSet, HeaderBar, FooterBar },

  props: {
    json: { type: Object, default: () => new Object() },
  },
};
</script>
