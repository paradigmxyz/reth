{
	// called once
	setup: function(cfg) {
		//<setup>
	},
	// required function that is invoked when a step fails
	fault: function(log, db) {
		//<fault>
	},
	// required function that returns the result of the tracer: empty object
	result: function(ctx, db) {
		//<result>
	},
	// optional function that is invoked for every opcode
	step: function(log, db) {
		//<step>
	},
	enter: function(frame) {
		//<enter>
	},
	exit: function(res) {
		//<exit>
	}
}