{
	// required function that is invoked when a step fails
	fault: function(log, db) { },
	// required function that returns the result of the tracer: empty object
	result: function(ctx, db) { return {}; },
	// optional function that is invoked for every opcode
	step: function(log, db) { }
}