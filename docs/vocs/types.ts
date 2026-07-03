export type SidebarItem = {
    badge?: string | {
        text: string;
        variant?: "note" | "info" | "warning" | "danger" | "tip" | "success";
    };
    collapsed?: boolean;
    disabled?: boolean;
    external?: boolean;
    items?: SidebarItem[];
    link?: string;
    text?: string;
};
