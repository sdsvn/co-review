import { ColorContribution } from '@theia/core/lib/browser/color-application-contribution';
import { ColorRegistry } from '@theia/core/lib/browser/color-registry';
import { FrontendApplicationContribution } from '@theia/core/lib/browser/frontend-application-contribution';
import { inject, injectable } from '@theia/core/shared/inversify';
import { MonacoThemingService } from '@theia/monaco/lib/browser/monaco-theming-service';

/**
 * The Co-Review brand: ink and indigo for the workbench, mint for the agent (and approval).
 * Same palette as the logo (docs/assets/logo.svg) and the phone view.
 */
export const BRAND = {
    ink: '#1E1B4B',
    indigo: '#6366F1',
    indigoSoft: '#A5B4FC',
    mint: '#34D399',
    mintDeep: '#059669'
};

const darkColors: Record<string, string> = {
    'foreground': '#D6D8F0',
    'descriptionForeground': '#9C9BC4',
    'focusBorder': BRAND.indigo,
    'widget.shadow': '#0008',
    'selection.background': '#3B3A7A',
    'textLink.foreground': BRAND.indigoSoft,
    'textLink.activeForeground': '#C7D2FE',
    'button.background': BRAND.indigo,
    'button.hoverBackground': '#7C7FF5',
    'button.foreground': '#FFFFFF',
    'button.secondaryBackground': '#2A2952',
    'button.secondaryForeground': '#D6D8F0',
    'badge.background': BRAND.indigo,
    'badge.foreground': '#FFFFFF',
    'activityBar.background': '#0F0E20',
    'activityBar.foreground': '#D6D8F0',
    'activityBar.inactiveForeground': '#6E6C9E',
    'activityBar.activeBorder': BRAND.indigo,
    'activityBarBadge.background': BRAND.mint,
    'activityBarBadge.foreground': '#064E3B',
    'titleBar.activeBackground': '#0F0E20',
    'titleBar.inactiveBackground': '#0F0E20',
    'statusBar.background': '#0F0E20',
    'statusBar.foreground': '#C7C9E8',
    'statusBar.noFolderBackground': '#0F0E20',
    'statusBar.border': '#24234A',
    'statusBarItem.hoverBackground': '#24234A',
    'sideBar.background': '#131226',
    'sideBar.border': '#24234A',
    'sideBarSectionHeader.background': '#131226',
    'sideBarTitle.foreground': '#9C9BC4',
    'panel.background': '#131226',
    'panel.border': '#24234A',
    'editor.background': '#17162B',
    'editor.foreground': '#D6D8F0',
    'editor.lineHighlightBackground': '#1E1D38',
    'editor.selectionBackground': '#3B3A7A',
    'editor.inactiveSelectionBackground': '#2C2B5A',
    'editor.findMatchHighlightBackground': '#34D39933',
    'editorLineNumber.foreground': '#4B4A78',
    'editorLineNumber.activeForeground': BRAND.indigoSoft,
    'editorCursor.foreground': BRAND.mint,
    'editorIndentGuide.background1': '#24234A',
    'editorWidget.background': '#1C1B36',
    'editorWidget.border': '#2E2D57',
    'editorHoverWidget.background': '#1C1B36',
    'editorHoverWidget.border': '#2E2D57',
    'editorGroupHeader.tabsBackground': '#131226',
    'editorGroup.border': '#24234A',
    'tab.activeBackground': '#17162B',
    'tab.inactiveBackground': '#131226',
    'tab.activeForeground': '#FFFFFF',
    'tab.inactiveForeground': '#8A89B8',
    'tab.activeBorderTop': BRAND.indigo,
    'tab.border': '#131226',
    'editorGutter.addedBackground': BRAND.mint,
    'editorGutter.modifiedBackground': '#818CF8',
    'editorGutter.deletedBackground': '#F87171',
    'diffEditor.insertedTextBackground': '#34D39926',
    'diffEditor.removedTextBackground': '#F8717126',
    'input.background': '#1C1B36',
    'input.border': '#2E2D57',
    'input.placeholderForeground': '#6E6C9E',
    'dropdown.background': '#1C1B36',
    'dropdown.border': '#2E2D57',
    'quickInput.background': '#1C1B36',
    'list.activeSelectionBackground': '#2A2860',
    'list.activeSelectionForeground': '#FFFFFF',
    'list.inactiveSelectionBackground': '#232250',
    'list.hoverBackground': '#1F1E3D',
    'list.highlightForeground': BRAND.mint,
    'list.focusOutline': BRAND.indigo,
    'scrollbarSlider.background': '#6366F133',
    'scrollbarSlider.hoverBackground': '#6366F155',
    'scrollbarSlider.activeBackground': '#6366F177',
    'menu.background': '#1C1B36',
    'menu.selectionBackground': '#2A2860',
    'notifications.background': '#1C1B36',
    'progressBar.background': BRAND.indigo,
    'charts.purple': '#A78BFA',
    'coReview.agent': BRAND.mint
};

const lightColors: Record<string, string> = {
    'foreground': BRAND.ink,
    'descriptionForeground': '#5B5A7E',
    'focusBorder': BRAND.indigo,
    'selection.background': '#C7D2FE',
    'textLink.foreground': '#4F46E5',
    'textLink.activeForeground': '#3730A3',
    'button.background': '#4F46E5',
    'button.hoverBackground': '#4338CA',
    'button.foreground': '#FFFFFF',
    'button.secondaryBackground': '#E0E7FF',
    'button.secondaryForeground': BRAND.ink,
    'badge.background': '#4F46E5',
    'badge.foreground': '#FFFFFF',
    'activityBar.background': '#EEEEF8',
    'activityBar.foreground': BRAND.ink,
    'activityBar.inactiveForeground': '#8B8BA7',
    'activityBar.activeBorder': BRAND.indigo,
    'activityBarBadge.background': BRAND.mintDeep,
    'activityBarBadge.foreground': '#FFFFFF',
    'titleBar.activeBackground': '#EEEEF8',
    'titleBar.inactiveBackground': '#EEEEF8',
    'statusBar.background': BRAND.ink,
    'statusBar.foreground': '#E0E7FF',
    'statusBar.noFolderBackground': BRAND.ink,
    'statusBarItem.hoverBackground': '#312E81',
    'sideBar.background': '#F6F6FB',
    'sideBar.border': '#E4E5F2',
    'sideBarSectionHeader.background': '#F6F6FB',
    'sideBarTitle.foreground': '#5B5A7E',
    'panel.background': '#F6F6FB',
    'panel.border': '#E4E5F2',
    'editor.background': '#FFFFFF',
    'editor.foreground': BRAND.ink,
    'editor.lineHighlightBackground': '#F5F6FF',
    'editor.selectionBackground': '#C7D2FE',
    'editor.inactiveSelectionBackground': '#E0E7FF',
    'editor.findMatchHighlightBackground': '#34D39940',
    'editorLineNumber.foreground': '#A5A8C8',
    'editorLineNumber.activeForeground': '#4F46E5',
    'editorCursor.foreground': '#4F46E5',
    'editorWidget.background': '#FFFFFF',
    'editorWidget.border': '#D6D8EE',
    'editorHoverWidget.background': '#FFFFFF',
    'editorHoverWidget.border': '#D6D8EE',
    'editorGroupHeader.tabsBackground': '#F6F6FB',
    'editorGroup.border': '#E4E5F2',
    'tab.activeBackground': '#FFFFFF',
    'tab.inactiveBackground': '#F6F6FB',
    'tab.activeForeground': BRAND.ink,
    'tab.inactiveForeground': '#6E6C9E',
    'tab.activeBorderTop': BRAND.indigo,
    'tab.border': '#F6F6FB',
    'editorGutter.addedBackground': '#10B981',
    'editorGutter.modifiedBackground': BRAND.indigo,
    'editorGutter.deletedBackground': '#EF4444',
    'diffEditor.insertedTextBackground': '#10B98126',
    'diffEditor.removedTextBackground': '#EF444426',
    'input.background': '#FFFFFF',
    'input.border': '#D6D8EE',
    'input.placeholderForeground': '#A5A8C8',
    'dropdown.background': '#FFFFFF',
    'dropdown.border': '#D6D8EE',
    'list.activeSelectionBackground': '#E0E7FF',
    'list.activeSelectionForeground': BRAND.ink,
    'list.inactiveSelectionBackground': '#EEF0FF',
    'list.hoverBackground': '#F1F2FC',
    'list.highlightForeground': '#4F46E5',
    'scrollbarSlider.background': '#6366F126',
    'scrollbarSlider.hoverBackground': '#6366F144',
    'scrollbarSlider.activeBackground': '#6366F166',
    'progressBar.background': BRAND.indigo,
    'charts.purple': '#7C3AED',
    'coReview.agent': BRAND.mintDeep
};

type TokenRule = { scope: string | string[]; settings: { foreground?: string; fontStyle?: string } };

function tokens(c: { comment: string; string: string; keyword: string; fn: string; type: string; constant: string; punct: string; tag: string; attr: string }): TokenRule[] {
    return [
        { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: c.comment, fontStyle: 'italic' } },
        { scope: ['string', 'markup.inline.raw'], settings: { foreground: c.string } },
        { scope: ['keyword', 'storage', 'storage.type', 'keyword.control'], settings: { foreground: c.keyword } },
        { scope: ['entity.name.function', 'support.function', 'meta.function-call'], settings: { foreground: c.fn } },
        { scope: ['entity.name.type', 'entity.name.class', 'support.type', 'support.class'], settings: { foreground: c.type } },
        { scope: ['constant', 'constant.numeric', 'constant.language', 'variable.language'], settings: { foreground: c.constant } },
        { scope: ['punctuation', 'meta.brace', 'keyword.operator'], settings: { foreground: c.punct } },
        { scope: ['entity.name.tag'], settings: { foreground: c.tag } },
        { scope: ['entity.other.attribute-name', 'support.type.property-name'], settings: { foreground: c.attr } },
        { scope: ['markup.heading', 'entity.name.section'], settings: { foreground: c.keyword, fontStyle: 'bold' } },
        { scope: ['markup.inserted'], settings: { foreground: c.string } },
        { scope: ['markup.deleted'], settings: { foreground: '#F87171' } }
    ];
}

export const CO_REVIEW_THEMES = [
    {
        id: 'co-review-dark', label: 'Co-Review Dark', uiTheme: 'vs-dark' as const,
        json: {
            colors: darkColors,
            tokenColors: tokens({ comment: '#6E6C9E', string: '#6EE7B7', keyword: '#C4B5FD', fn: '#7DD3FC', type: '#FCD34D', constant: '#F9A8D4', punct: '#9C9BC4', tag: '#A5B4FC', attr: '#FDBA74' })
        }
    },
    {
        id: 'co-review-light', label: 'Co-Review Light', uiTheme: 'vs' as const,
        json: {
            colors: lightColors,
            tokenColors: tokens({ comment: '#8B8BA7', string: '#047857', keyword: '#6D28D9', fn: '#1D4ED8', type: '#B45309', constant: '#BE185D', punct: '#6B6B8A', tag: '#4338CA', attr: '#C2410C' })
        }
    }
];

/** Registers the brand themes before the workbench restores the user's theme. */
@injectable()
export class CoReviewThemeContribution implements FrontendApplicationContribution, ColorContribution {

    @inject(MonacoThemingService) protected readonly theming: MonacoThemingService;

    initialize(): void {
        for (const theme of CO_REVIEW_THEMES) {
            this.theming.registerParsedTheme(theme);
        }
        // The browser tab shows the logo (the phone view serves the same icon).
        const icon = document.createElement('link');
        icon.rel = 'icon';
        icon.type = 'image/svg+xml';
        icon.href = '/m/icon.svg';
        document.head.append(icon);
    }

    registerColors(colors: ColorRegistry): void {
        colors.register({
            id: 'coReview.agent',
            defaults: { dark: BRAND.mint, light: BRAND.mintDeep, hcDark: BRAND.mint, hcLight: BRAND.mintDeep },
            description: 'Agent messages, questions and activity in reviews.'
        });
    }
}
