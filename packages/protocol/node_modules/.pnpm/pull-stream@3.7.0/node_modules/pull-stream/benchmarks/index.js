const Benchmark = require('benchmark');

Benchmark.invoke(
    [
        require('./flatten'),
        require('./node'),
        require('./pull')
    ],
    'run'
);

