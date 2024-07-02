"use strict";
// Convert camelCase strings to various other formats  -- assumes input field is camelCase or camel1Case
Object.defineProperty(exports, "__esModule", { value: true });
exports.Case = void 0;
exports.Case = {
    snake: (field) => field
        .replace(/[^0-z]/g, "")
        .replace(/[a-z][A-Z]|[0-9][A-Z]/g, (substr) => substr[0] + "_" + substr[1].toLowerCase()),
    constant: (field) => field
        .replace(/[^0-z]/g, "")
        .replace(/[a-z][A-Z]|[0-9][A-Z]/g, (substr) => substr[0] + "_" + substr[1])
        .toUpperCase(),
    pascal: (field) => {
        const first = field[0].toUpperCase();
        return (first + field.slice(1)).replace(/[^0-z]/g, "");
    },
    camel: (field) => {
        return field[0].toLowerCase() + field.slice(1);
    },
    header: (field) => {
        const first = field[0].toUpperCase();
        return (first +
            field
                .slice(1)
                .replace(/[^0-z]/g, "")
                .replace(/[a-z][A-Z]|[0-9][A-Z]/g, (substr) => substr[0] + "-" + substr[1]));
    },
    eth2: (field) => exports.Case.snake(field).replace(/(\d)$/, "_$1"),
};
//# sourceMappingURL=strings.js.map