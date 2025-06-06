import 'package:flutter/widgets.dart';
import 'package:universal_platform/universal_platform.dart';

class GridSize {
  static double scale = 1;

  static double get scrollBarSize => 8 * scale;

  static double get headerHeight => 36 * scale;

  static double get buttonHeight => 38 * scale;

  static double get footerHeight => 36 * scale;

  static double get horizontalHeaderPadding =>
      UniversalPlatform.isDesktop ? 40 * scale : 16 * scale;

  static double get cellHPadding => 10 * scale;

  static double get cellVPadding => 8 * scale;

  static double get popoverItemHeight => 26 * scale;

  static double get typeOptionSeparatorHeight => 4 * scale;

  static double get newPropertyButtonWidth => 140 * scale;

  static double get mobileNewPropertyButtonWidth => 200 * scale;

  static EdgeInsets get cellContentInsets => EdgeInsets.symmetric(
        horizontal: GridSize.cellHPadding,
        vertical: GridSize.cellVPadding,
      );

  static EdgeInsets get compactCellContentInsets =>
      cellContentInsets - EdgeInsets.symmetric(vertical: 2);

  static EdgeInsets get typeOptionContentInsets => const EdgeInsets.all(4);

  static EdgeInsets get toolbarSettingButtonInsets =>
      const EdgeInsets.symmetric(horizontal: 6, vertical: 2);

  static EdgeInsets get footerContentInsets => EdgeInsets.fromLTRB(
        GridSize.horizontalHeaderPadding,
        0,
        UniversalPlatform.isMobile ? GridSize.horizontalHeaderPadding : 0,
        UniversalPlatform.isMobile ? 100 : 0,
      );

  static EdgeInsets get contentInsets => EdgeInsets.symmetric(
        horizontal: GridSize.horizontalHeaderPadding,
      );
}
